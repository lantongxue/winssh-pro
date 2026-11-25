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

**如实说明收益边界**：头两处（`Box` 化的 tile 参考、内联提示）是安全、可回归的优化，
但量级有限（约一成上下）。当时把剩下的大头——标量逆 DWT、i64 定点 YCbCr→RGB、位串行
RLGR——一并记成了"codec 固有成本"。**前两项这个判断是错的，第七轮已推翻**（见下面
"重建路径向量化"）：逆 DWT 的"循环携带依赖"只是上游那种写法带来的，算法本身没有；
i64 也只是为了躲溢出，换个拆法 i32 就够。仍然成立的只有 RLGR：它逐位读码流，下一个
符号从哪一位开始取决于上一个符号解出了什么，这才是真的串行。

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
纯 eager，只保留下面的性能优化与 profiling。当时的结论是"progressive 滚动 CPU 是 codec
固有成本，软件侧空间已探尽"，详见 docs 第六轮。**这个结论下早了**：跨帧合并这条路确实
走不通，但单个 tile 的重建本身还能便宜（下一节）。服务器肯发 H.264 时那仍然是最省的路
（VideoToolbox / Media Foundation 解码，代码侧已就绪），只是不该再拿它当"软件侧没得做"
的理由。

**重建路径向量化（第七轮）**。`reconstruct_to_rgba` 每个 tile 做三件事：反量化、三个
分量各一次三级逆 DWT、4096 个像素的 YCbCr→RGB。后两件上游都是一次一个样本的标量循环，
且都**不是算法要求的**：

- *逆 DWT*。LeGall 5/3 的逆变换是两步提升：`even[j] = low[j] − (high[j−1] + high[j]) / 2`
  只读输入；`odd[j] = (even[j] + even[j+1]) / 2 + 2·high[j]` 只读已经算完的偶数样本。
  同一步里的样本彼此不依赖。上游把两步揉进一趟循环、用寄存器接力上一个偶数样本和高频
  系数，才造出了"循环携带依赖"；列方向还是按步长跨行跳着读。改成两步各自对整段切片做
  逐元素映射：行方向是两次映射加一次交织，列方向干脆"一整行当一个样本"——所有列齐步走，
  跨行跳读也变成了顺序读。两种形状编译器都能自动向量化。
- *算术留在 i16*。上游在 i32 里算、最后 `as i16` 截断；截断与 wrapping 加减、移位可交换，
  所以只有"除以 2"要小心（i16 里先加会溢出，且上游是向零取整不是向下取整）：
  `half_sum` 用 `(a & b) + ((a ^ b) >> 1)` 得到不溢出的向下取整，和为负奇数时补回 1。
  16 位通道意味着 SSE2 一条指令 8 个样本、AVX2 16 个，而不是 i32 的一半。
- *色彩转换*。上游为了让 `cr * 91881` 这类乘积在整个 i16 输入范围内不溢出而升到 i64；
  没有哪个 SIMD 目标能便宜地做 64 位整数乘法，这个循环就一直是标量。把每个系数在 2^16
  的整数倍处拆开（`91881 = 65536 + 26345`、`46802 = 65536 − 18734`、
  `116130 = 2·65536 − 14942`），整数倍那部分出了 `>> 16` 就是一次加法，剩下要乘的落在
  i16 以内，全范围最坏情形 `32768 × (22554 + 18734) ≈ 1.35e9` 仍在 i32 里。

三处都**逐位等于上游**，不是"误差在容忍范围内"：progressive 的 tile 每个 refinement pass
都拿同一份系数重建一次，差一个灰阶的像素会在两个值之间跳，看上去就是闪。所以上游的标量
实现原样留在 `#[cfg(test)]` 里当对照，新实现逐项与它比对（含会回绕的满幅 i16 输入）。

没有用 `std::arch` 手写指令，也没有 `unsafe`：只是把循环改成编译器看得懂的样子。因此
各目标（x86_64 基线 SSE2、aarch64 NEON）自己拿各自的宽度，不需要按 CPU 分发。

**实测收益**（Windows 11 x86_64，release，2026-11-25）。量尺是
`reconstruct_cost_against_the_upstream_scalar_path`：510 个 tile（1920×1080 一整帧的量，
64 种不同内容轮着用，免得分支预测器背下来），同一进程里上游路径与本副本**交替**跑 31 轮
取中位数，顺带断言两边逐字节相等。跑了 5 遍：

| | 一整帧 510 tile | 每 tile |
| --- | --- | --- |
| 上游标量路径（反量化 + 标量逆 DWT + i64 色彩转换） | 26.1 – 27.6 ms | ≈ 52 µs |
| 本副本 | 7.1 – 7.5 ms | ≈ 14 µs |

**×3.6 – 3.7。** 对照侧（`reconstruct_upstream`）是把上游 0.11.0 的三步原样拼回来的：
`decode_scalar` 串起三级标量逆 DWT，色彩转换用 i64 的原式和原来的按下标循环。

这是**进程内的重建量尺**，不含 RLGR 熵解码（没动，仍是串行的那部分），也不是真实会话
的 CPU：写这段时手边没有可连的 RDP 服务器，下面"验证"一节的 `live_scroll` 没跑。第五轮
实机采样里重建约占会话线程的七成，据此估计滚动时会话线程 CPU 能降一半上下 ——
**这是推算，不是实测**，要人工连一台真机确认。

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `src/progressive.rs` | `ProgressiveDecoder::references` 与 `decode_tile_block` 的参数从 `BTreeMap<_, DecDwtQ>` 改为 `BTreeMap<_, Box<DecDwtQ>>`：节点只存 8 字节指针，插入/红黑树调整不再 memmove 24KB。新增 `store_reference`：已知 tile 原地覆写（复用堆分配），仅首见 tile 分配，稳态滚动不产生 24KB alloc/free 抖动。三处 `references.insert(...)` 改走它；两处 `.get(...)` 读参考补 `.as_ref()`。跟随的测试解引用改 `**` / `.map(\|b\| **b)`。语义不变，61 项 progressive 测试全过。 |
| `src/dwt_extrapolate.rs` | 解码路径换成按提升步骤拆开的实现：新增 `half_sum`（i16 内的向零取整半和）、`lift_even` / `lift_odd`（两步提升各一个逐元素映射）、`idwt_row_lifted`（行：两次映射 + 交织）、`idwt_columns_lifted`（列：整行为单位做同样两步）；`decode_block` 改调它们。上游的 `decode_block` 原样改名 `decode_block_scalar`，连同 `idwt_row` / `idwt_col` 一起降为 `#[cfg(test)]` 的对照实现（`#[inline]` 留着：它们不在解码路径上了，但量尺拿它们当"改之前"，得和当初发出去的那版一样快才公平）。另加 `#[cfg(test)] pub(crate) fn decode_scalar`：把三级标量逆 DWT 按 `decode` 的顺序串起来，给整 tile 对照测试和 progressive 的量尺用。编码侧（`encode_block` 等）未动。新增 4 项测试：`half_sum_matches_the_widened_division`、`idwt_row_lifted_matches_the_scalar_lifter`、`decode_block_matches_the_scalar_lifter`、`decode_matches_the_scalar_lifter_on_whole_tiles`，系数幅度从贴近真实（`>> 6`）到满幅回绕（`>> 0`）。 |
| `src/progressive.rs`（色彩转换） | `ycbcr_to_rgb` 从 i64 改为系数拆分后的 i32，`clamp_u8` 随之收 `i32`；`reconstruct_to_rgba` 末尾的像素循环从按下标取三个平面改为 `as_chunks_mut::<4>` 与三个平面 zip（无下标运算、无边界检查）。新增测试 `ycbcr_to_rgb_matches_the_widened_formula`：红、蓝两路各对色度穷举 65536 个值，绿路在互质步长的网格上遍历，亮度特意取能抵消色度项的值——让通道落在 0..255 中间，否则低位算错会被饱和钳位盖住。另加测试辅助 `reconstruct_upstream`（上游三步原样拼回）与量尺 `reconstruct_cost_against_the_upstream_scalar_path`（`#[ignore]`，数见上）。 |
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
cargo test --manifest-path vendor/ironrdp-graphics/Cargo.toml --release   # 257 项通过，1 项 ignored（量尺）
cargo test --release -p winssh-rdp -p winssh-desktop -- --test-threads=1  # 87 + 11 全过

# 重建量尺（新旧交替取中位数，机器吵的话多跑几遍看区间）
cargo test --manifest-path vendor/ironrdp-graphics/Cargo.toml --release     reconstruct_cost -- --ignored --nocapture
```

单独跑这份副本会在 `vendor/ironrdp-graphics/target/` 下留构建产物。这个目录早先被误提交
进了仓库（1396 个文件），跑测试会改到其中的 `.rustc_info.json`；`.gitignore` 现已忽略它，
但已入库的那批要另行 `git rm -r --cached` 才算清干净。想绕开就加
`CARGO_TARGET_DIR=target/vendor-graphics`。

实机量尺（不自动启动应用）：`crates/winssh-rdp/src/tests/session/live.rs`
（`WINSSH_RDP_PROF=1 cargo test --release -p winssh-rdp live_scroll -- --ignored --nocapture`），
分段耗时见会话线程每秒一行的 `RDP prof` 日志。