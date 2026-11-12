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
