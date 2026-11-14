# 这份 `ironrdp-graphics` 与上游的差别

`vendor/ironrdp-graphics` 逐字来自 crates.io 的 **ironrdp-graphics 0.11.0**
（<https://github.com/Devolutions/IronRDP>，MIT OR Apache-2.0），根 `Cargo.toml`
的 `[patch.crates-io]` 指着它。它是 `ironrdp-session` / `ironrdp-egfx` 的解码底座，
RemoteFX（classic + progressive）、ClearCodec、Planar、ZGFX 都在这。

## 为什么要动它

实机采样（macOS arm64，release，`sample` 抓会话线程，快速滚动网页）显示 CPU 几乎
全部落在 **RemoteFX progressive 解码**：`progressive::TileState::reconstruct_to_rgba`
→ `dwt_extrapolate::decode_block/idwt_row`（标量逆 DWT）、RLGR/SRL 熵解码，以及
`BTreeMap::insert` 搬 24KB 的 `DecDwtQ` tile 参考值。服务器（无 H.264 硬编的虚拟机）
协商到 GFX V8_1 不带 AVC，滚动内容走 progressive，所以这条解码路就是 CPU 大头。
上游 0.11 的解码器是纯标量、tile 参考用 `BTreeMap<_, DecDwtQ>` 按值存，没有"升级即修"
的路，故在副本上做数据结构与内联优化。

**如实说明收益边界**：下面两处是安全、可回归的优化，但量级有限（约一成上下）。
剩下的大头是标量逆 DWT（循环携带依赖，难以 SIMD）、i64 定点 YCbCr→RGB（ARM NEON 无
64 位整数乘法，SIMD 收益差）与位串行 RLGR——这些是 codec 固有成本。

**实测纠正一个想当然**：曾以为"同一 tile 一帧里被多个 pass 反复重建，延迟到帧末只重建
一次能省九成"。实机量（`WINSSH_RDP_PROF=1`，见下）得 **recon/tile ≈ 1.00**——每个 tile
每帧**已经只重建一次**（升级 pass  refine 的是上一帧的 tile，裁剪循环也不重复重建已解
tile）。所以"延迟到帧末"收益为零。真正可能有收益的是**跨帧合并**：会话线程把每一帧都
解码（熵解码必须逐帧做，系数跨帧累积），但呈现层（`Presentation` 的 QUIET/MAX_BATCH +
FrameSender 单帧合并）会丢弃中间帧；只重建被呈现那帧的 tile 才能省。收益 ≈ 丢帧率 ×
重建占比（约 70%），**若服务器发帧率 ≤ 呈现帧率则也为零**。

**跨帧合并：试过，残影，实验代码已全部撤回**。实机快滚量得 `coalesce ≈ 2~3`，曾 vendor
`ironrdp-egfx` 加呈现级延迟重建旁路、本 crate 配套加 `decode_bitmap_lazy` / `reconstruct_tile`。
单帧测试逐像素一致，但**实机多帧快滚残影**：progressive 每帧 refine 整块 tile 系数、只标该帧
子区，攒多帧矩形最后用最终系数重建一次会 retroactively 改写早先帧区域（eager 每帧用各自系数
贴各自矩形）。多帧回归实证 eager≠lazy。且滚动时每帧整屏 tile 都在变（recon/tile≈1.00），
跨帧合并即便修对收益也有限。**证伪后已全部撤回**：egfx 副本删除、本 crate 的 lazy 出口
（`decode_bitmap_lazy`/`reconstruct_tile`/`reconstruct_pixels` 的 lazy 分支）移除，解码器回到
纯 eager，只保留下面的性能优化与 profiling。结论：progressive 滚动 CPU 是 codec 固有成本，
软件侧空间已探尽，**到 mstsc 级低 CPU 的正路是服务器发 H.264**（VideoToolbox 硬解，代码侧
已就绪）。详见 docs 第六轮。

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `src/progressive.rs` | `ProgressiveDecoder::references` 与 `decode_tile_block` 的参数从 `BTreeMap<_, DecDwtQ>` 改为 `BTreeMap<_, Box<DecDwtQ>>`：节点只存 8 字节指针，插入/红黑树调整不再 memmove 24KB。新增 `store_reference`：已知 tile 原地覆写（复用堆分配），仅首见 tile 分配，稳态滚动不产生 24KB alloc/free 抖动。三处 `references.insert(...)` 改走它；两处 `.get(...)` 读参考补 `.as_ref()`。跟随的测试解引用改 `**` / `.map(\|b\| **b)`。语义不变，61 项 progressive 测试全过。 |
| `src/dwt_extrapolate.rs` | `idwt_row` / `idwt_col` 加 `#[inline]`：逆 DWT 内层提升循环留在寄存器里，去掉每行/每列的调用开销。纯内联提示，不改算法。 |
| `src/progressive.rs`（profiling） | 新增私有 `mod prof`：两个累计原子计数（`reconstruct_to_rgba` 次数、`end_frame` 帧数），仅 `WINSSH_RDP_PROF=1` 时累加，否则零开销（`OnceLock<bool>` 门）。导出 `progressive_recon_count()` / `progressive_frame_count()` 供上层 `winssh-rdp` 的 `Prof` 读取，换算解码帧率/重建率，与呈现帧率对比量跨帧合并收益。不改解码语义。 |
## 解码帧率 / 重建率量具

`winssh-rdp` 的 `Prof`（`WINSSH_RDP_PROF=1`）每秒一行里有三个字段（诊断用）：

- `decode_fps`：本秒解码的 GFX 帧数（`end_frame` 计数增量）。
- `recon/s`：本秒 progressive 重建次数（`reconstruct_to_rgba` 计数增量）。
- `coalesce`：`decode_fps / publishes`（publishes 是呈现帧率）。> 1 说明呈现层丢帧。

曾想用 coalesce>1 论证跨帧合并延迟重建有收益，实机量得 2~3 后建了，**但残影证伪**（见上）。
教训：coalesce 高是**呈现层丢帧**，不代表解码层有冗余可省——progressive 滚动时每帧整屏 tile
都在变（recon/tile≈1.00），被呈现那帧仍要重建全部脏 tile。这几个计数保留作诊断。

## 验证

```sh
cargo test --manifest-path vendor/ironrdp-graphics/Cargo.toml --release   # 252 项全过
cargo test --release -p winssh-rdp -p winssh-desktop -- --test-threads=1  # 79 + 11 全过
```

实机量尺（不自动启动应用）：`crates/winssh-rdp/src/tests/session/live.rs`
（`WINSSH_RDP_PROF=1 cargo test --release -p winssh-rdp live_scroll -- --ignored --nocapture`），
分段耗时见会话线程每秒一行的 `RDP prof` 日志。