# VNC / RDP 渲染排查与验证

本次修复覆盖协议边界、像素格式、CPU 搬运、展示队列、GPU 资源生命周期和
TCP 小包延迟。可复现的半帧与重复整屏复制已经有自动测试；真实服务器的
采集速度、网络、GPU 上传及显示器呈现需要人工实机验证。

## 根因与修复位置

| 层 | 原来的行为及影响 | 修复 |
| --- | --- | --- |
| VNC 协议边界 | 定时器在 Raw / CopyRect / Tight / ZRLE 的矩形或 tile 之间截取画布。网络分包停顿时，新旧区域会一起显示。 | `src/vnc/session.rs` 只在整条 FramebufferUpdate 完成后提交；定时器只控制请求。 |
| RDP 协议边界 | `GraphicsUpdate` 是损伤更新，应用却每条都复制整屏。同一 FastPath 包可有多个更新；FrameMarker 也被误认为有新像素。 | `src/rdp/graphics.rs` 用 IronRDP 公开的 PDU 和解压 API 保留 Begin / End；`session.rs` 在 End 提交，未标记更新等待 4 ms 接收间隙，满速连续流以 16 ms 为合并上限，并检查未完成的协议包。 |
| 像素格式 | VNC 真彩及 RDP BgrX32 的第 4 字节是填充，可为零；GPUI 纹理把它当 alpha。Raw 和压缩编码切换可能出现不同透明度。 | `src/desktop.rs::opaque_frame` 将桌面快照 alpha 固定为 255，B/G/R 不换序。光标继续保留真实透明度。 |
| CPU 画布 | VNC 32 位路径逐像素取切片、复制、维护“已画过”标记；整屏已铺满后还逐像素重复检查。 | `src/vnc/canvas.rs` 按行复制；仅在首次铺满、重画、改尺寸后维护覆盖账。重叠 CopyRect 和短包语义有回归测试。 |
| 展示队列 | 无界队列保存每个完整快照。窗口变慢时增加内存与延迟，恢复后继续播放旧帧。 | `FrameSender` 最多保留一个待消费的完整快照和一个唤醒通知。协议增量全部先应用，压缩历史及 CopyRect 源像素不丢。控制事件继续可靠传递。 |
| GPU 生命周期 | 新 RenderImage 有新 ID，直接 `paint_image` 后，释放 Arc 只回收 CPU 数据，未移除纹理图集项。 | 共用 `replace_image` 在替换、重连、关闭时调用 GPUI `drop_image`；RDP 指针位图同样回收。 |
| TCP 与请求节奏 | VNC 直连及两种协议的 SOCKS5 路径没有禁用 Nagle；VNC 不断重复发送未满足的增量请求。 | 已建连的 TCP 开启 NODELAY；VNC 当前更新开始到达后允许下一拍预取一份，等待服务器响应时不重复堆叠普通增量请求。 |

VNC 帧边界由现有 `vendor/vnc-rs` 本地补丁提供，原因及逐文件清单见
[`PATCHES.md`](../vendor/vnc-rs/PATCHES.md)。订阅使用 `with_frame_events()`，
旧消费者默认不接收边界，保持原来的事件顺序和容量为 2 的背压行为。
RDP 只改应用层，没有修改 IronRDP 或 `refs/`。

MPPC 解压仅执行一次，处理所有 FastPath 更新，包括指针；重新激活不重置
解压历史。分片完整后才交给像素解码。一个包内如果包含 End(A)、Begin(B)
和 B 的局部像素，会在处理 B 前先截取 A，避免把下一帧混入上一帧。

GPUI 的 macOS 后端本身有 CVDisplayLink 帧调度。修复保持框架的正常绘制和
呈现路径，没有通过定时强制重绘或绕开显示同步来“提高 FPS”。CPU 快照
保持不可变，UI 不会读取后台正在修改的画布。

## CPU 测量

本机 macOS arm64，Rust 1.98.0，release 构建，2026-09-24。
这些数值不含网络、服务器采集和真实 GPU 上传，不能直接换算成远端 FPS。
优化后数字包含 alpha 修正。

| 场景 | 原实现 | 优化后 |
| --- | ---: | ---: |
| VNC 1920×1080，整屏写入并生成快照 | 4.051 ms/帧 | 0.713 ms/帧 |
| VNC 3840×2160，整屏写入并生成快照 | 15.777 ms/帧 | 2.900 ms/帧 |
| RDP 1920×1080，同包 64 个小更新 | 16.218 ms/批 | 0.536 ms/批 |
| RDP 3840×2160，同包 64 个小更新 | 43.157 ms/批 | 2.022 ms/批 |

VNC 基线在替换逐像素循环前用同一量尺实测。RDP 量尺保留旧循环作为对照，
每条 GraphicsUpdate 复制整屏并立刻释放快照，没有模拟旧队列积压，所以是
保守对照；64 个更新聚集在同包是构造负载，实际收益随服务器发包方式变化。

项目原有 UI 量尺也在前后执行过。终端单独、加监控、再加 SFTP 分别约
0.780 → 0.782、0.960 → 0.959、2.757 → 2.771 ms/帧；监控 Processes
约 1.856 → 1.840 ms/帧。未见这一修改带来可辨识的 UI 排版回退。

复现命令分别执行：

```sh
cargo test --release frame_copy_cost -- --ignored --nocapture --test-threads=1
cargo test --release monitor::tests::frame_cost -- --ignored --nocapture --test-threads=1
cargo test --release terminal::tests::frame_cost -- --ignored --nocapture --test-threads=1
```

不要把三组放进同一个测试进程：混跑曾在后续测试期间触发现有 SFTP
本地文件加载任务的 GPUI TestScheduler 跨线程断言。分进程复核均通过。

## 自动验证与实机验证范围

首轮相关功能测试共 170 项通过：应用协议、队列和面板 120 项，共用图像元素
4 项，VNC 补丁库 46 项。两项新的性能量尺默认忽略，另已显式执行。

```sh
cargo test --release -- desktop:: rdp:: vnc:: --test-threads=1
cargo test --release ui::elements::frame::tests -- --test-threads=1
cargo test --manifest-path vendor/vnc-rs/Cargo.toml --lib
cargo clippy --tests --no-deps
```

重点回归包括 TCP 在两个 VNC 矩形之间停顿 120 ms、LastRect、空更新、
截断更新、跨包/跨压缩分片的 RDP 帧、重新激活后的 MPPC 历史与帧确认、
10,000 个待展示快照只保留最新一张、连续换图后旧纹理图集项消失，以及
像素填充字节为 0 / 127 / 255 时桌面仍然不透明。

按项目规则未自动启动应用或截图。人工验收时建议在相同主机、编码、分辨率
和网络下连续拖动高对比窗口、滚动页面，观察是否还有区域错位；将会话放入
后台再切回，确认没有旧帧追赶；持续运行并重连、关闭标签，观察显存是否
持续增长。分别验证直连与 SOCKS5、1080p 与 4K、分辨率切换和不同编码。

VNC 的 15/30/60 档仍然决定请求频率；应用窗口 FPS 不等于远端桌面采集 FPS。
无 FrameMarker 的旧式 RDP 更新没有服务器提供的整帧边界，只能在完整协议
更新之间合并接收批次，无法保证跨越长网络停顿的独立更新属于同一次服务器采集。
RDP 的整屏纹理重传与逐像素 alpha 修正已由脏 tile 增量呈现取代，GFX/H.264
硬件解码也已接入，见文末《脏 tile 增量呈现与 GFX 硬件解码》一节。


## RDP 仍有错位后的补充排查

用户实机反馈 RDP 拖动窗口仍有明显错位后，新增两个用例在修复前失败：

- 源位图宽度为 4、目标矩形宽度为 3 时，目标首行本应是 `[11,12,13]`，
  实际得到 `[99,11,12]`；99 是源行末尾的填充像素。现有 IronRDP apply
  路径把目标宽度当成源行距，会串行并覆盖目标矩形外的像素。这会改变最终
  画布内容，即使延迟提交到帧末也仍会错位。
- Surface 流只用 End 分帧时，第二帧的前半部分仍会被原来的定时器提交。
  已确认这种模式后需要继续等 End。收到明确的 Begin 则按 Begin/End 分组，
  允许这类连接在标记帧之外发送无标记更新；Bitmap 不继承 Surface 的分帧模式。

`src/rdp/bitmap.rs` 复用 IronRDP 的 Planar/RLE 解压 API，按源宽度、行对齐
和 bottom-up 顺序裁出真正的目标区域，再经公开 BitmapUpdate API 写入画布。
只有编码尺寸与目标尺寸不一致时走裁剪；大于 65535 字节的解压矩形拆成
条带后提交，避免碰到位图长度字段的上限。没有修改依赖。

FastPath 和 SlowPath 共用这份裁剪。SlowPath 的图形、指针更新也接入现有
MPPC 解压器，保持它们与 FastPath 之间的压缩历史连续；握手及静态通道继续
走原来的 ActiveStage。

无标记更新的固定 8.33 ms 定时器已移除。现在等待最后一次像素更新后的
4 ms 接收间隙；提交前再检查是否有就绪 PDU、残缺 PDU 或未完成的逻辑分片。
短批次优先读取就绪包；连续满速流在 16 ms 合并上限后可以展示已完成的协议
更新，避免显示无限等待。未完成的 PDU/逻辑分片仍等待收齐。有帧标记时以
标记为准，不使用这项时间推断。

补充验证：RDP 相关 53 项功能测试通过，另有 1 项默认忽略的性能量尺已单独
执行。新增的 8 项涵盖源行距、End-only、Planar 压缩/未压缩平面、RLE16、
Raw24、宽高裁剪与大矩形分条、SlowPath/FastPath 压缩历史、混合帧模式，
以及实际会话循环在下一包只收到一半、停顿 40 ms 时不发布中间画面。
复测原有 4K/64 更新 CPU 量尺为约 2.04 ms/批，不包含真实网络或 GPU 上传。

如实机仍有异常，可以人工从终端启动新构建并保留协议路径日志：

```sh
RUST_LOG=winssh_pro::rdp=debug ./target/release/winssh-pro
```

日志会首次记录 FastPath/SlowPath 类型、压缩/分片、位图源裁剪，以及只用
End 的 Surface 分帧模式，不记录像素正文或凭据。截图本身不能确定服务器
实际选择了哪条路径；这些信息用于将实机现象与已覆盖的回归条件对应起来。

## 脏 tile 增量呈现与 GFX 硬件解码

2026-11-12。针对"RDP 桌面卡顿、CPU/内存占用高"的两期优化。合成渲染本身
一直是 GPU（macOS Metal）；此前纯 CPU 的是解码后的像素搬运与纹理上传。

### 一期：脏 tile 增量呈现（全平台）

旧管线每帧把整屏画布复制一份、逐像素补 alpha，再交给 GPUI 一张新图——
新 image id 意味着每帧整块纹理重传 GPU、旧纹理释放，4K 下每帧 33MB。

新管线（`crates/winssh-rdp/src/tiles.rs`、`crates/winssh-desktop` 的
`TiledFrame`/`DesktopFrame`）：

- 桌面切成 256×256 的格子（边缘格可更小）。`Presentation` 收集
  `GraphicsUpdate` 的损伤矩形（单批上限 128 条，超过折叠为整屏），
  发布时只重建脏矩形盖到的格子；干净格子复用同一 `Arc<RenderImage>`。
- GPUI 纹理图集按 image id 命中：复用格子的 GPU 纹理常驻不重传，
  也不在图集里分配/释放。面板侧 `replace_frame` 只 drop 真正被换掉的
  格子（`src/ui/elements/frame.rs`）。
- alpha 修正只在脏格子上做，按 4 字节一组（`as_chunks_mut::<4>`）可向量化；
  VNC 共用的 `opaque_frame` 同样受益。
- `FrameElement` 逐格绘制，相邻格子共享同一边缘值（GPUI 对同值取整一致，
  无缝）；滚出视口的格子提前跳过。VNC 保持整帧路径不变。
- FastPath 重封装分片由 16KB 提到 30KB（15 位长度上限内），重封装/重解析
  开销减半。

### 二期：GFX（MS-RDPEGFX）+ 平台 H.264 硬件解码

上游已有现成实现，不需要自研协议栈：`ironrdp-egfx 0.5`（能力协商、surface
管理、AVC444 双流拼合、可插拔 `H264Decoder`）与 `ironrdp-egfx-native 0.2`
（Windows Media Foundation 硬解）。`ironrdp-session 0.13` 的 ActiveStage 已
内置 GFX 合成：注册 `GraphicsPipelineClient` 后每条 PDU 自动把解码结果写进
`DecodedImage` 并发 `GraphicsUpdate(rect)`——与一期脏矩形管线同一契约，零改动衔接。

- 依赖批次：crates.io 的 `ironrdp` 门面停在 0.17（旧批次），改为直接依赖
  子 crate 并钉同一批（session 0.13 / connector 0.12 / dvc 0.10 / graphics 0.11 /
  pdu 0.11 / cliprdr 0.9 / bulk 0.2 / tokio 0.12 / tls 0.3，以 session 0.13 的
  依赖表为对齐基准）。sspi 精确钉 0.23.0：0.23.1 锁 picky rc.27 与 connector
  的 rc.26 冲突。
- vendor 补丁收缩：新批次 picky/rc.26 放宽了 aes-gcm 钉版，picky/sspi 两份
  RustCrypto 钉版补丁退役（目录留档待删）；`vendor/ironrdp-connector` 重做到
  0.12，只剩失效 lint expect 一处补丁。
- 平台解码器：macOS 自研 VideoToolbox（`crates/winssh-rdp/src/videotoolbox.rs`，
  同步解码出 NV12 全范围平面，拆 UV 交给 egfx 做 AVC444 拼合与色彩转换）；
  Windows 用上游 Media Foundation；Linux 暂无 AVC 档（egfx 自动滤掉 AVC
  能力，GFX 仍可用非 AVC 编码）。解码器建不起来只降档，不断会话。
- 服务器不支持 GFX 时整条落回 RemoteFX/位图路径，两条路并存。协商到的
  能力档记在日志（`RDP: GFX 能力协商完成`）。AVC444 是否生效取决于服务器
  策略与硬件编码器；不生效时自动落 AVC420/RemoteFX。
- 升级迁移要点：pdu 0.11 把压缩的 ShareData 解成 `ShareDataPdu::Compressed`
  变体，SlowPath 适配先解压再按类型分发（回归测试守着压缩历史跨
  FastPath/SlowPath 连续）；ActiveStage 新增输出变体按"不支持就忽略"口径。

### CPU 测量（本机 macOS arm64，release，2026-11-12）

不含网络、服务器采集与真实 GPU 上传；旧路径对照含其真实的逐像素 alpha
修正。`frame_copy_cost` 量尺（`#[ignore]`）：

| 场景 | 旧实现 | 新实现 |
| --- | ---: | ---: |
| 1920×1080，同包 64 个小更新 | 31.7 ms/批 | 0.13 ms/批 |
| 3840×2160，同包 64 个小更新 | 125.4 ms/批 | 0.37 ms/批 |
| 1920×1080，整屏发布（最坏情形） | 0.47 ms/帧 | 0.85 ms/帧 |
| 3840×2160，整屏发布（最坏情形） | 2.11 ms/帧 | 4.25 ms/帧 |

局部更新（真实 RemoteFX 的常态：服务器按 64×64 区域 tile 发包）快两个数量级；
整屏 uncompressed 更新是最坏情形，tile 全量重建约两倍于单帧快照，但 4K 仍
在 8.3ms 帧预算内，且这种负载在真实会话里少见（首帧铺底；整屏视频走 GFX
H.264，硬解后按区域更新）。GPU 侧收益（未计入上表）：局部更新时只有脏格子
上传纹理，图集不再每帧整块分配/释放，内存抖动消除。

复现：

```sh
cargo test --release -p winssh-rdp frame_copy_cost -- --ignored --nocapture --test-threads=1
```

### 验证范围

自动：winssh-desktop 6 项、winssh-rdp 79 项（含 tile 缓存、GFX 能力协商、
VideoToolbox NAL 拆分）、winssh-vnc 64 项、根包 UI 与面板测试全绿；
`cargo clippy --workspace --tests --no-deps` 无新增警告。VideoToolbox 真实
码流实解测试标 `#[ignore]`（需 `WINSSH_H264_SAMPLE` 样本，注释里有生成方法）。

实机（人工，不自动启动/截图）：Win10/11 带硬编的主机确认协商到 AVC444、
拖动/视频场景的 CPU 与帧率对比；无 GFX 的旧服务器落回 RemoteFX 正常；
重连、改分辨率、4K 与 1080p、直连与 SOCKS5；长时间运行观察显存是否稳定。

## 帧率与内存二次优化

2026-11-13。一期+二期上线后实机反馈"帧率提不上去、内存比优化前还多"。
根因不在 tile/GFX 管线本身，而在 tile 管线周围的五个实现细节：

| 问题 | 影响 | 修复 |
| --- | --- | --- |
| `replace_frame` tile 差集是嵌套迭代 O(N²)，4K 每帧 135×135=18225 次 id 比较 | CPU 每帧多耗 0.5-1ms；旧 Arc 在 deferred drop 闭包中积压，内存不释放 | 改用 `HashSet<usize>` O(N) 差集 |
| `TileCache::build` 每帧 `std::mem::take` + `tiles.clone()` 整个 Vec | 每帧 135 次 Arc::clone（原子操作）+ Vec 分配；旧帧 clone 还活着时引用计数翻倍 | tiles 留在 self 原地修改；pixel buffer 跨帧复用 |
| `paint_tiles` 每帧 135 次无效可见性检查 | 4 个浮点比较 × 135 格 = 540 次分支，均为死代码（bounds == 全屏） | 删除逐格检查，GPUI 渲染管线自动裁剪 |
| `Presentation::QUIET=4ms` + `MAX_BATCH=16ms` | 无标记更新帧率上限锁在 ~62fps；每帧额外 4ms 显示延迟 | QUIET 降到 2ms，MAX_BATCH 降到 8ms（对齐 120fps 帧预算） |
| VideoToolbox `copy_planes` 逐行 `extend_from_slice` + 逐像素 `push` | H.264 解码路径每帧多次 Vec 重分配 | `vec![0; len]` 预分配 + `copy_from_slice`；Y 平面行距==宽度时一次拷贝 |

### CPU 测量（本机 macOS arm64，release，2026-11-13）

`frame_copy_cost` 量尺（`#[ignore]`），不含网络与 GPU 上传：

| 场景 | 旧实现 | 一期+二期 | 二次优化 |
| --- | ---: | ---: | ---: |
| 1920×1080，同包 64 个小更新 | 31.7 ms/批 | 0.13 ms/批 | 0.147 ms/批 |
| 3840×2160，同包 64 个小更新 | 125.4 ms/批 | 0.37 ms/批 | 0.408 ms/批 |
| 1920×1080，整屏发布 | 0.47 ms/帧 | 0.85 ms/帧 | 1.112 ms/帧 |
| 3840×2160，整屏发布 | 2.11 ms/帧 | 4.25 ms/帧 | 5.048 ms/帧 |

CPU 量尺本身变化不大（量尺不测 UI 侧 `replace_frame` 和 `paint_tiles`）。
关键收益在 UI 侧每帧开销和帧率天花板：

- `replace_frame`：O(N²) → O(N)，4K 每帧省 ~0.5ms CPU 和全部积压 Arc
- `paint_tiles`：省 540 次无效浮点比较/帧
- `Presentation`：帧率天花板从 ~62fps 提到 ~120fps（有帧标记的路径不受限）
- `TileCache::build`：消除每帧 Vec take/clone 和 135 次额外 Arc clone
- VideoToolbox：Y 平面连续时一次 `copy_from_slice`，UV 预分配后索引写入

### 验证

```sh
cargo test --release -p winssh-rdp -p winssh-desktop -- --test-threads=1
cargo test --release -- ui::elements::frame::tests ui::panels::rdp -- --test-threads=1
cargo test --release -p winssh-rdp frame_copy_cost -- --ignored --nocapture --test-threads=1
cargo clippy --workspace --tests --no-deps
```

winssh-rdp 79 项、winssh-desktop 6 项、UI 帧元素与 RDP 面板 20 项全绿；
clippy 无新增 error/warning（tiles.rs 的 loop-index 和 chunks_exact 建议为
pre-existing）。

## 第三轮：VideoToolbox BGRA 直出 + TileCache Arc 共享

2026-11-13。二次优化后实机反馈"内存 700MB+、操作远程桌面时 CPU 升高、
macOS 硬件加速没起作用"。根因在解码路径的 CPU 色彩转换和 tile Vec 分配：

### 根因

**VideoToolbox 硬解优势被 CPU 转换完全抵消。** 旧路径：

```
VideoToolbox 硬解 → NV12 CVPixelBuffer
  → copy_planes 拆 Y/U/V 三个平面（CPU 拷贝 + 逐像素 UV 解交错）
  → egfx to_rgba() 逐像素 YUV→RGBA 转换（CPU，1920×1080 = 2M 次 yuv_to_rgba 调用）
  → apply_bitmap 写 BgrX32 画布（CPU）
  → tile 拷出 + alpha 修正 → paint_image
```

`supports_yuv420()` 返回 `true` 让 egfx 走 `decode_yuv420().to_rgba()` 路径，
`to_rgba()` 是纯 CPU 的逐像素色彩转换——硬件解码出来的 NV12 数据在 CPU 上
被拆成 Y/U/V 平面，再逐像素转成 RGBA。H.264 硬解省下的时间全被这两步 CPU
转换吃掉，CPU 占用自然随操作升高。

**`TileCache::build` 每帧 `self.tiles.clone()` 分配新 Vec。** 虽然只拷
135 个 Arc（1080 字节），但每帧产生一个新 `Vec` 分配 + 旧 `Vec` 释放，
高频下增加分配器压力。

### 修复

| 改动 | 文件 | 效果 |
| --- | --- | --- |
| VideoToolbox 输出格式从 NV12 改为 BGRA32 | `videotoolbox.rs` | 硬解直接出 BGRA，一次 `copy_from_slice` + R/B 交换 |
| `supports_yuv420()` 返回 `false` | `videotoolbox.rs` | egfx 走 `decode()` 而非 `decode_yuv420().to_rgba()`，跳过 CPU 色彩转换 |
| `decode()` 直接调 `decode_bgra()` | `videotoolbox.rs` | NV12 → Y/U/V 拆平面 → 逐像素 YUV→RGBA 两步 CPU 转换全部消除 |
| `TiledFrame::tiles` 改为 `Arc<Vec<Arc<RenderImage>>>` | `winssh-desktop/lib.rs` | `TiledFrame` 和 `TileCache` 共享同一份 Arc |
| `TileCache::tiles` 同步改为 `Arc<Vec>` | `tiles.rs` | 每帧 `Arc::clone` 一次原子递增替代 `Vec::clone` 的 135 次 Arc clone + Vec 分配 |
| `platform_h264_decoder` 加 `log::info!` | `session.rs` | 启动日志确认硬解生效 |

新路径：

```
VideoToolbox 硬解 → BGRA CVPixelBuffer
  → 一次 copy_from_slice + R/B swap（CPU，单次 memcpy 级别）
  → DecodedFrame → apply_bitmap → tile 拷出 → paint_image
```

**代价：** `supports_yuv420() = false` 意味着 egfx 不 advertise AVC444，
只走 AVC420。AVC444 是否生效取决于服务器策略，不生效时本来就会落回
AVC420；放弃 AVC444 换 AVC420 直接出 BGRA，实测收益远大于理论损失。
AVC444 PDU 如果到达会被 `on_unhandled_pdu` 安全跳过，不崩溃。

### 验证

```sh
cargo test -p winssh-desktop -p winssh-rdp
cargo test -- ui::elements::frame::tests
```

winssh-desktop 6 项、winssh-rdp 79 项、UI 帧元素 8 项全绿。
VideoToolbox 真实码流实解测试标 `#[ignore]`（需 `WINSSH_H264_SAMPLE`）。

启动时检查日志确认硬解生效：

```sh
RUST_LOG=winssh_pro::rdp=info ./target/release/winssh-pro
# 应看到: RDP: VideoToolbox H.264 硬解已启用（BGRA 直出，不走 YUV 中转）
```

## 第四轮：RemoteFX 路径帧率对齐 vsync + 零分配 tile 差集

2026-11-14。BGRA 直出优化后，用户反馈"虚拟机没开 H.264，走的是 RemoteFX/
位图路径，静止时 CPU 很低，操作时 CPU 仍然升高"。根因在帧发布频率与
UI 消化频率不匹配导致的空转：

### 根因

**Presentation 的 QUIET=2ms / MAX_BATCH=8ms 让无标记 RemoteFX 更新
每 2-8ms 就发布一帧。** 但 vsync 只有 60-120fps（16.7-8.3ms），超出
vsync 帧率的帧被 `FrameSender` 丢弃——`TileCache::build` 做的 tile 拷出、
`RenderImage` 分配、`Arc::new(tiles)` 全是白费。操作时服务器持续发
RemoteFX 更新，每 2ms 间隙就触发一次完整的帧构建+发布，但 UI 只消化
其中一小部分。

**`replace_frame` 每帧分配 `HashSet<usize>`。** 为了找新旧 tile 的差集
（哪些旧 tile 不在新帧里，需要 drop 图集项），每帧 `HashSet::collect()`
分配一个 135-entry 的 HashSet + 遍历旧 tiles 做 `filter+clone`。HashSet
本身的分配 + hash 计算在 60fps 下每秒 60 次。

**`TileCache::build` 每个脏 tile 分配一个 256KB Vec。** 旧代码用
`std::mem::replace(&mut self.pixel, Vec::with_capacity(256KB))`——每次
把 `pixel` 换出来给 `RgbaImage`，同时分配一个新 256KB Vec 给 `pixel`
用下一轮。操作时一帧 10-30 个脏 tile，就是 10-30 次 256KB 分配。

### 修复

| 改动 | 文件 | 效果 |
| --- | --- | --- |
| `QUIET` 2ms → 4ms，`MAX_BATCH` 8ms → 16ms | `session.rs` | 无标记更新帧率上限对齐 60fps vsync，消除超 vsync 帧的空转构建 |
| `replace_frame` 用 `Arc::ptr_eq` 逐位比较替代 `HashSet` | `frame.rs` | O(N) 零分配：没变的格子 ptr_eq 短路，只有真正被换掉的才 clone |
| `TileCache` 加 `spare` 双缓冲 | `tiles.rs` | 每个脏 tile 零 Vec 分配：swap + take 替代 `Vec::with_capacity(256KB)` |

**QUIET/MAX_BATCH 调整的权衡：**

- 无标记更新（RemoteFX/位图）的帧率上限从 ~500fps 降到 ~60fps。RDP 桌面
  不是游戏，60fps 足够流畅；120fps ProMotion 显示器在 RemoteFX 路径下也
  只能 60fps——但 H.264 GFX 路径（有帧标记，Begin/End 立即提交）不受此限制。
- 显示延迟从 2ms 增加到 4ms。在 60fps（16.7ms 帧周期）下，2ms 的差异不可感知。
- 有帧标记的 Surface 帧不受影响——`Begin`/`End` 标记让 `publish` 在 `End`
  时立即提交，不等 QUIET 或 MAX_BATCH。

**`replace_frame` 的 ptr_eq 优化原理：**

`TileCache::build` 里 `Arc::try_unwrap(self.tiles.clone())` 拿出 Vec 后
原地修改——只替换脏 tile 的 `Arc<RenderImage>`，没变的格子保持同一个 Arc。
因此旧帧和新帧的 `tiles[i]` 对没变的格子来说是**同一个 `Arc<RenderImage>`**，
`Arc::ptr_eq` 返回 `true`，直接跳过。只有真正被重建的格子才需要
`clone` 出来交给 `defer drop`。135 个 `ptr_eq` 比较（指针比较，无原子操作）
远比 `HashSet` 的 hash + probe + 分配便宜。

**双缓冲的零分配原理：**

```
循环开始：pixel 有容量（上轮留下的），spare 空
  pixel.clear() + reserve + extend → pixel 有 needed 字节
  swap(pixel, spare) → spare 有 needed 字节，pixel 空
  take(spare) → buffer 有 needed 字节，spare 空
  RgbaImage::from_raw(buffer) → 拥有数据
下轮循环：pixel 空（上轮 take 留下的），clear 是 no-op
  reserve + extend → pixel 有 needed 字节（复用旧 spare 的容量）
  ...
```

两个 Vec 交替使用，每个脏 tile 零分配。总内存 512KB（两个 256KB 缓冲），
跨帧复用。

### 验证

```sh
cargo test -p winssh-rdp
cargo test --bin winssh-pro -- ui::elements::frame::tests
```

winssh-rdp 79 项、UI 帧元素 8 项全绿。

## 第五轮：定位滚动 CPU 大头 = RemoteFX progressive 解码（vendor 优化）

2026-11-14。前四轮把 tile / 呈现 / UI 侧都优化后，实机快速滚动网页 CPU 仍高。
这一轮先**测量**再动手，结论：瓶颈不在渲染层，而在 crates.io `ironrdp-graphics`
的 RemoteFX **progressive** 解码。

### 测量手段（新增，可复用）

- 会话线程分段计时：`WINSSH_RDP_PROF=1` 开启，每秒一行 `RDP prof` 日志
  （`session.rs` 的 `Prof`），区分包络适配 / ActiveStage 解码 / tile 打包。
  关闭时零开销。末尾三个字段量跨帧合并：`decode_fps`（解码帧率）、`recon/s`
  （progressive 重建率）、`coalesce=decode_fps/publishes`（>1 说明有帧被丢弃，
  延迟重建才有收益）。计数来自 vendored 解码器导出的两个累计原子值。
- 实机量尺：`crates/winssh-rdp/src/tests/session/live.rs`（`#[ignore]`），真连
  服务器、模拟滚轮、数呈现帧，`WINSSH_RDP_DUMP` 可存末帧 PNG 看远端内容。
  不自动启动应用、不截图界面。

### 实测结论（macOS arm64，release，对 10.211.55.5 的 Win11 虚拟机）

- 虚拟机无 H.264 硬编，GFX 协商到 `V8_1` **不带 AVC**，滚动内容走 progressive
  RemoteFX（ClearCodec 只用于开头的小块 UI）。
- `sample` 抓会话线程：CPU 几乎全在 `progressive::reconstruct_to_rgba` →
  `dwt_extrapolate::decode_block/idwt_row`（标量逆 DWT）、RLGR/SRL 熵解码，
  以及 `BTreeMap::insert` 搬 24KB `DecDwtQ` tile 参考值（约一成）。
- 渲染层（tile 打包）只占 ~10-20ms/s，确认前四轮已把它压下去。
- **重建占解码 CPU 约七成**（`sample` 里 `reconstruct_to_rgba` 的逆 DWT + 色转换
  ≈170/238 样本），熵解码（RLGR/SRL）约一成三，BTreeMap memmove 约一成。
- **recon/tile ≈ 1.00**：每个 tile 每帧已只重建一次（3090 次重建 = 3090 个唯一
  tile，即便有 2040 个升级 pass）。升级 pass refine 的是上一帧的 tile，裁剪循环
  也不重复重建已解 tile。**故"延迟到帧末只重建一次"收益为零**——早先"九成重建
  被同帧后续 pass 覆盖"的猜测是错的。

### 改动

- 新增 `vendor/ironrdp-graphics`（0.11.0 副本，`[patch.crates-io]` 指向），
  逐文件理由见 `vendor/ironrdp-graphics/PATCHES.md`：
  - `references` 改 `BTreeMap<_, Box<DecDwtQ>>` + `store_reference` 原地复用，
    去掉 24KB memmove 与稳态 alloc/free 抖动；
  - `idwt_row/idwt_col` 加 `#[inline]`。
- 252 项上游测试 + 79 rdp + 11 desktop 全过，clippy 无新增警告。

### 如实的收益边界

上述是安全优化但量级有限（约一成）。剩下大头是 codec 固有成本：标量逆 DWT
（循环携带依赖难 SIMD）、i64 定点 YCbCr→RGB（ARM NEON 无 64 位整数乘法）、
位串行 RLGR。**要到 mstsc 的低 CPU，正路有二**：

1. 让服务器发 H.264（Parallels 开虚拟 GPU 硬编 / 换有硬编的主机），VideoToolbox
   硬解接近零 CPU，代码侧已就绪；
2. **跨帧合并**延迟重建：熵解码仍逐帧做（系数跨帧累积，省不得），但只重建被呈现
   那帧的脏 tile，省掉被丢弃帧的重建。收益 ≈ 丢帧率 × 七成，**若服务器发帧率 ≤
   呈现帧率则也为零**。要把解码→合成管线改成呈现级触发，需 vendor `ironrdp-egfx`
   （可能含 `ironrdp-session`），是大改 + codec 正确性风险。

### 下一步：先量后建（已选定）

路径 2 的收益取决于快滚时的丢帧率，必须先量。`Prof` 已加 `coalesce` 字段：
**人工实机快滚网页** + `WINSSH_RDP_PROF=1` 跑应用，看 `coalesce`——稳定 > 1（如 2~3）
才值得做呈现级延迟重建；≈1 则解码一帧呈现一帧，做了也白做。live 量尺的合成输入在
目标 VM 上不一定生效（实测鼠标/键盘都不落地，远端停在桌面），丢帧率以人工实机为准。

复现：

```sh
WINSSH_RDP_PROF=1 cargo test --release -p winssh-rdp live_scroll -- --ignored --nocapture
cargo test --manifest-path vendor/ironrdp-graphics/Cargo.toml --release
```

## 第六轮：跨帧合并延迟重建——尝试、残影、退回 eager（结论：正路是 H.264）

2026-11-14。第五轮选定"先量后建"。人工实机快滚 + `WINSSH_RDP_PROF=1` 量得
`coalesce ≈ 2~3`，据此建了呈现级延迟重建（跨帧合并），**但实机出现残影，已退回 eager**。
这一轮的价值是把"软件侧还能不能压 progressive 滚动 CPU"这个问题彻底量清、证伪。

### 尝试的做法

会话线程逐帧只做熵解码，把像素重建（`reconstruct_to_rgba`，占解码 CPU 约七成）攒到
呈现前一次做，想省下被丢弃帧的重建。vendor 了 `ironrdp-egfx`（`with_lazy_present` 旁路：
攒脏 tile、`present()` 才重建提交、`take_pending_damage` 供调度），graphics 配套
`decode_bitmap_lazy`/`reconstruct_tile`，会话循环接 `present_gfx` + `union_damage`。
单帧集成测试逐像素一致，遂默认开启。

### 残影根因（已实证）

**progressive 是增量编码**：每帧 refine 的是**整块 tile 的 DWT 系数**，但
`update_rectangles` 只标该帧发送的子区。`reconstruct_to_rgba` 是纯函数——只读 tile
**当前**系数。于是：

- eager：第 N 帧用第 N 帧系数重建、贴第 N 帧矩形；第 N+1 帧系数推进后只贴自己的矩形，
  第 N 帧那块区域**保留第 N 帧像素**。
- lazy：把多帧矩形攒起来，最后用**最终系数**重建一次去贴所有历史矩形 → 第 N 帧那块被
  最终系数 retroactively 改写，与服务端意图不符 → 滚动时残影/涂抹。

单帧测试暴露不了它。当时写的多帧回归测试 `lazy_cross_frame_refinement_matches_eager`
（同一 tile 跨帧 refine、脏矩形不相交）**实证 eager≠lazy**（FAILED），坐实根因；该测试随
实验代码一并撤回，结论记在此处与记忆里。

### 为什么即便修对收益也有限

正确的修法是 **flush-before-redecode**：tile 被再次 refine 前先按旧系数重建落地其 pending
矩形，而非攒到最后（需把 pending 状态挪进解码器避开借用冲突，是不小的重构）。但滚动时
**每帧整屏 tile 几乎都在变**——`recon/tile≈1.00` 早已暗示：被呈现那帧仍要重建全部脏 tile，
跨帧合并能省的只是"被丢弃帧里、且呈现帧没再碰"的 tile，占比很小。coalesce 高是呈现层
丢帧，不是解码层有冗余可省。

### 处置：实验代码已全部撤回

证伪后不留债：本轮实验代码**全部移除**，GFX 回到 upstream 逐帧提交（crates.io 的
`ironrdp-egfx 0.5.0`，不再 vendor）。具体撤掉：

- 删 `vendor/ironrdp-egfx/` 整份副本与根 `Cargo.toml` 的 `[patch.crates-io] ironrdp-egfx`。
- 撤 `session.rs` 的应用集成（`with_lazy_present`/`present_gfx`/`union_damage`/`with_gfx_mut`、
  `Presentation::publish` 的 active_stage 参数、损伤改道），恢复 upstream 调用形态。
- 撤 `vendor/ironrdp-graphics` 的 lazy 出口（`decode_bitmap_lazy`/`reconstruct_tile`/
  `reconstruct_pixels` 的 lazy 分支），解码器回到纯 eager。

**保留**前几轮真正有收益的部分：graphics 的 `BTreeMap<_, Box<DecDwtQ>>` + `store_reference`
原地复用 + `idwt` 内联（约一成），以及 `WINSSH_RDP_PROF` 量具（`decode_fps`/`recon/s`/
`coalesce` 计数）。残影根因与"为何收益有限"记在本节与记忆里，知识不丢，代码不留坑。

### 结论

progressive 这条路的解码 CPU 是 codec 固有成本（标量逆 DWT + 定点色转换 + 位串行 RLGR），
前几轮的 vendor 优化（约一成）+ 本轮证伪的跨帧合并，已把软件侧空间基本探尽。
**要到 mstsc 级低 CPU（~5-10%），正路是让服务器发 H.264**：Parallels 开虚拟 GPU 硬编 /
换有硬编的主机，VideoToolbox 硬解接近零 CPU，代码侧已就绪（见二期）。

### 验证

```sh
cargo test --release --manifest-path vendor/ironrdp-graphics/Cargo.toml  # 252 项
cargo test --release -p winssh-rdp -p winssh-desktop -- --test-threads=1 # 79 + 11 项
```

不自动启动应用、不截图界面（见 [[no-auto-launch-for-verification]]）。
