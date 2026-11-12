# RDP 渲染性能优化：脏 tile 增量呈现 + RDPGFX 硬件解码

## 背景与结论

每帧整屏复制 + 逐像素 alpha 循环 + 整块纹理重传 GPU 是卡顿与 CPU/内存占用主因。分两期：

- **一期（tile 呈现）**：脏 tile 纹理缓存。GPUI 图集按 image id 命中，未变 tile 不重传。全平台立刻受益，VNC 保持整帧路径不变。
- **二期（RDPGFX 硬解）**：上游已有现成实现 —— `ironrdp-egfx 0.5`（完整 MS-RDPEGFX：能力协商、surface 管理、AVC444 双流拼合、可插拔 `H264Decoder` trait）与 `ironrdp-egfx-native 0.2`（Windows Media Foundation 硬解，支持 YUV420 平面输出即 AVC444）。`ironrdp-session 0.13` 的 ActiveStage 已内置 GFX 合成：注册 `GraphicsPipelineClient` 后每条 PDU 自动把解码结果写进 `DecodedImage` 并发 `GraphicsUpdate(rect)` —— 与一期脏矩形管线同一契约。**不需要自研协议栈**；缺口只有 macOS 的 VideoToolbox 解码器（上游 native crate 目前仅 Windows）。

前提修正：当前钉的 `ironrdp` 门面 0.17 对应旧批次（dvc 0.8 / session 0.11 / graphics 0.9），与 egfx 要求的 dvc 0.10 不兼容；门面最新版仍是 0.17，未跟上已发布的 session 0.13 批次，二期需改为直接依赖各子 crate 并手动对齐同一批次。GPUI `paint_surface` 仅 NV12，不作为桌面呈现路径。

---

# 一期：脏 tile 增量呈现

## winssh-desktop：共享帧类型（crates/winssh-desktop/src/lib.rs）

- 新增 `TiledFrame`：`width/height: u32`、`tile: u32`（256）、`cols/rows: u32`、`tiles: Vec<Arc<RenderImage>>`（行优先，边缘 tile 尺寸可小于 256）。提供 `size()` 与按格索引取 tile 的方法。
- 新增 `DesktopFrame` 枚举：`Whole(Arc<RenderImage>)` / `Tiled(Arc<TiledFrame>)`，统一 `size()`；`From<Arc<RenderImage>>` 便于 VNC 侧包装。
- `opaque_frame` 的 alpha 循环从 `pixels_mut()` 改为 `chunks_exact_mut(4)` 置 `p[3]=255`（可向量化，VNC 同受益）。

## winssh-rdp：脏矩形收集与 tile 缓存

- 新文件 `crates/winssh-rdp/src/tiles.rs`：`TileCache`
  - 持有当前 tile 网格 `Vec<Arc<RenderImage>>` 与桌面尺寸；尺寸变化（重激活/改分辨率）自动全量重建。
  - `build(&mut self, image: &DecodedImage, dirty: &[InclusiveRectangle]) -> Arc<DesktopFrame>`：由脏矩形算出脏 tile 集合；仅对脏 tile 按行从 `DecodedImage`（用 `stride()`）拷出、`chunks_exact_mut(4)` 修 alpha、生成新 `RenderImage`；干净 tile 复用原 Arc。首帧全量。
  - 纯函数拆出便于测试：网格划分、脏矩形→tile 下标映射（含边缘裁剪）、tile 像素矩形。
- `session.rs`：
  - `Presentation` 增加 `dirty_rects: Vec<InclusiveRectangle>`；`damage(now)` 改为 `damage(now, rect)`，两处调用点（会话循环 `GraphicsUpdate(rect)` 输出、`process_graphics_frame` 的 emit 闭包）把矩形传进来。矩形数超过 128 时折叠为整屏，防无界增长。
  - `publish`/`publish_due` 接收 `&mut TileCache`，产 `RdpOutput::Frame(Arc<DesktopFrame>)`（载荷类型由 `Arc<RenderImage>` 改为 `Arc<DesktopFrame>`），发布后清空脏矩形。`process_graphics_frame` 增加 `TileCache` 参数；会话结束时的收尾 publish 同路。
  - `process_complete_update` 的 `CHUNK` 由 16KB 提到 30KB（FastPath 长度字段 15 位上限 32767，留头余量），重封装/重解析开销减半。
- `lib.rs` 挂 `mod tiles;`，模块文档补一句 tile 呈现策略。

## 共享元素与面板（根包）

- `src/ui/elements/frame.rs`：
  - `FrameElement::new` 改收 `Arc<DesktopFrame>`；`Whole` 走原 `paint_image`；`Tiled` 逐 tile 画：tile 目标矩形按整体 bounds 比例折算，相邻 tile 共享同一边缘值（GPUI snap 一致，无缝），视口外的 tile 提前跳过。光标 overlay 逻辑不变（尺寸改从 `DesktopFrame::size()` 读）。
  - 新增 `replace_frame`：整帧替换沿用 `replace_image` 语义；tile 帧按 image id 差集，只 `drop_image` 真正被换掉的 tile（复用 tile 的图集项保留），覆盖重连/关闭/跨窗口。
- `src/ui/panels/rdp.rs`：`image` 字段改 `Option<Arc<DesktopFrame>>`，`set_frame` 用 `replace_frame`，分辨率从 `DesktopFrame::size()` 读；其余（输入换算、滚动、光标）不动。
- `src/ui/panels/vnc.rs`：仅把 `FrameElement::new(image, …)` 包成 `DesktopFrame::Whole`，行为不变。

## 测试（按项目规则外移，#[path] 挂回）

- `crates/winssh-rdp/src/tests/session/presentation.rs`：`Desktop::take()` 改返 `Arc<DesktopFrame>`，加一个把 tile 拼回整屏像素的测试助手，现有像素/时序断言保持不变。
- 新增 `crates/winssh-rdp/src/tests/tiles.rs`：脏 tile 才换新 id、干净 tile Arc 同一性；X=0 像素修成 255；非整除尺寸的边缘 tile；改尺寸全量重建；脏矩形→tile 映射纯函数；矩形超限折叠整屏。
- `src/tests/ui/elements/frame.rs`：补 tile 帧的图集回收用例（换掉的 tile 条目消失、复用的保留）与 tile 目标矩形纯函数（共享边缘无缝）；现有用例适配新签名。
- `src/tests/ui/panels/rdp.rs`：`set_frame` 两处调用适配 `DesktopFrame`。
- 性能量尺：扩展 `frame_copy_cost`（#[ignore]），对照整屏旧路径与 tile 路径在"64 个小更新"与"整屏更新"两种负载下的每批耗时，注释记数。

## 一期文档与验证

- 更新 `docs/remote-desktop-rendering.md`：tile 呈现管线与新量尺数据。
- 验证命令（分进程，遵循既有量尺规则）：
  - `cargo test --release -- desktop:: rdp:: --test-threads=1`
  - `cargo test --release ui::elements::frame::tests ui::panels::rdp -- --test-threads=1`
  - `cargo test --release frame_copy_cost -- --ignored --nocapture --test-threads=1`
  - `cargo clippy --tests --no-deps`
- 按项目规则不自动启动应用/截图；实机由人工验证（拖动窗口、滚动、4K、重连后显存是否稳定）。

## 一期预期收益与边界

- 典型桌面操作（局部脏区）：每帧 CPU 从整屏复制+830 万像素循环降到几个 tile；GPU 上传从整屏每帧降到仅脏 tile；图集不再每帧整块分配/释放，内存抖动消除。
- 整屏运动（视频）：上传总量不变，但 alpha 修正向量化、无整屏大分配；剩余瓶颈是 RLGR 解码与服务器编码 —— 由二期解决。
- 假设：tile 取 256×256（常量可调）；脏矩形上限 128；一期不修改 refs/ 与任何依赖版本。

---

# 二期：RDPGFX + 平台硬件 H.264 解码

## IronRDP 依赖批次升级（根 Cargo.toml + vendor）

- 弃用 `ironrdp` 门面（0.17 未跟上），`[workspace.dependencies]` 改列子 crate 并钉同一批次：`ironrdp-session 0.13`、`ironrdp-connector 0.12`、`ironrdp-dvc 0.10`、`ironrdp-graphics 0.11`、`ironrdp-pdu 0.11`、`ironrdp-core 0.3`、`ironrdp-svc 0.10`、`ironrdp-displaycontrol 0.10`、`ironrdp-bulk 0.2`、`ironrdp-cliprdr 0.9`、`ironrdp-tokio 0.12`、`ironrdp-tls 0.3`；新增 `ironrdp-egfx 0.5`。以 session 0.13 的依赖表为对齐基准，`cargo tree -d` 确认无重复批次。
- `vendor/ironrdp-connector` 补丁副本从 0.10 重做到 0.12：沿用两处补丁口径（picky default features 关闭、工具链 lint 修正），按 PATCHES.md 纪律更新逐文件改动表与字节一致性声明。
- API 迁移修复（`crates/winssh-rdp`）：ActiveStage 新增输出变体（MonitorLayout / WindowingOrders / WindowSurfaceUpdates / SaveSessionInfo 等）按"不支持就忽略"口径处理；connector Config、BulkCompressor、DisplayControl 的签名变化逐一适配。
- **重点回归 cliprdr 0.7→0.9**：文件复制状态机 API 可能变化（`file_copy.rs`、`clipboard.rs`），双向文件复制全量回归；若 0.9 已带 ChunkedFetch 可评估替换自研简化版（本次不强求）。
- 现有 RDP 53+ 项测试与 VNC/助手等共用铁律测试全量通过后才算升级完成。

## GFX 通道接线（crates/winssh-rdp/src/session.rs）

- `open()` 里向 `DrdynvcClient` 注册 `GraphicsPipelineClient::new(handler, decoder)`（与 DisplayControl 并列）：
  - handler 用默认实现（默认能力集已含 10.7→10 AVC444、8.1 AVC420、8 兜底）；
  - decoder 按平台：macOS 自研 VideoToolbox（下节）；Windows `ironrdp_egfx_native::MediaFoundationH264Decoder`（target 门控依赖）；Linux 暂传 `None`（egfx 自动滤掉 AVC 能力，GFX 仍可用非 AVC 编码，服务器不支持 GFX 时整条落回现有 RemoteFX 路径）。
- ActiveStage 自动合成 GFX 输出进 `DecodedImage` 并发 `GraphicsUpdate(rect)`，一期的脏矩形→tile 管线**零改动**复用；`ResetGraphics` 由 ActiveStage 处理画布重置，TileCache 按尺寸变化自动全量重建。
- 帧节奏：egfx 只在 EndFrame 后吐整帧增量，现有 4ms 间隙/16ms 合并启发式自然适配；GFX 激活时可选改为"批完成即发布"，以量尺决定。
- 解码线程化留作后续：session 0.13 提供 `composite_graphics_pipeline` 公开入口，支持把慢解码挪出会话循环（本次先同步解码，VideoToolbox/MF 硬解本身足够快）。

## macOS VideoToolbox 解码器（新文件 crates/winssh-rdp/src/videotoolbox.rs，target_os = "macos" 门控）

- 实现 `ironrdp_egfx::decode::H264Decoder`：`decode_yuv420` + `supports_yuv420() = true`（AVC444 必需），`decode` 可由 YUV 转换兜底。
- 要点：从码流解析 SPS/PPS 建 `CMVideoFormatDescription`；Annex B 与 4 字节长度前缀两种 NAL 封装都收（上游文档明确两者都会出现）；`VTDecompressionSession` 同步解码出 NV12 `CVPixelBuffer`，锁平面拷进 `Yuv420Frame`（Y/U/V 平面 + stride）；`reset()` 重建会话；`wait_until_ready` 处理异步初始化。
- 绑定走 objc2（项目已有 objc2 三件套先例），新增 `objc2-video-toolbox` / `objc2-core-video` / `objc2-core-media` 按 target 引入；unsafe 块逐处留 SAFETY 注释。
- 硬解失败/不可用时返回 Err，egfx 按能力协商落回非 AVC 档，不断会话。

## 二期测试与验证

- 升级回归：`cargo test --release -- rdp:: vnc:: desktop:: --test-threads=1` 全绿；剪贴板/文件复制双向用例重点跑。
- 新增：GFX 注册与能力协商单测（构造 PDU 序列喂 GraphicsPipelineClient，验证无解码器时滤 AVC、有解码器时 advertise AVC444）；VideoToolbox 解码器用真实 H.264 样本的测试标 `#[ignore]`（环境敏感，注释记条件）。
- 实机人工验证（不自动启动/截图）：Win10/11 带硬编的主机确认协商到 AVC444（日志记能力版本）、拖动/视频场景 CPU 与帧率对比、无 GFX 的旧服务器落回 RemoteFX 正常、重连与改分辨率。
- 文档：`docs/remote-desktop-rendering.md` 增补 GFX 管线、平台解码矩阵（macOS VideoToolbox / Windows MF / Linux 暂无 AVC）、升级批次记录。

## 二期风险与边界

- 最大风险是依赖批次升级的 API 漂移（尤其 cliprdr 文件复制），以现有测试全绿为闸门；GFX 接线本身很薄。
- AVC444 是否生效取决于服务器策略与硬件编码器；不生效时自动落 AVC420/RemoteFX，收益递减但不会更差。
- Linux 硬解（VAAPI）与 openh264 软解兜底（注意 Cisco 二进制许可，上游推荐 libloading 由用户自行下载）列为后续，不在本次。
- 不修改 refs/；vendor 补丁仅限 ironrdp-connector 既有口径的重做。