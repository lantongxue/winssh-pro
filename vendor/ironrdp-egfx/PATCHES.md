# 这份 `ironrdp-egfx` 与上游的差别

`vendor/ironrdp-egfx` 逐字来自 crates.io 的 **ironrdp-egfx 0.5.0**
（<https://github.com/Devolutions/IronRDP>，MIT OR Apache-2.0），根 `Cargo.toml`
的 `[patch.crates-io]` 指着它。拷进来时与 registry 里那一份 `diff -r` 无差别，
之后只动了 `src/decode.rs` 一个文件。

> 第六轮也 vendor 过一次 `ironrdp-egfx`，那是为"跨帧合并延迟重建"做的实验，残影证伪后
> 已整份删除（见 `docs/remote-desktop-rendering.md` 第六轮）。**这一份和那次没有关系**：
> 不改任何协议行为、不改解码时机，只是把一个逐像素的色彩转换循环换成逐位相等的快写法。

## 为什么要动它

服务器肯发 H.264 时（GFX AVC420），解码本身交给平台（Windows 上是 Media Foundation，
`ironrdp-egfx-native`），解出来的是 YUV 4:2:0 三个平面。把它变成能上屏的 RGBA 靠
`Yuv420Frame::to_rgba` —— **每一帧、整幅画面**都要过一遍。上游的写法是一个像素一次：

- 每个像素各做两次带边界检查的色度行查找（`u_row` / `v_row` 返回 `Option`）；
- 每个像素把色度项重新算一遍 —— 但 4:2:0 的一个色度样本管 2×2 四个像素，四次算的是同一个数；
- 每个像素一次 `Vec::extend_from_slice` 把 4 个字节推进输出（长度检查 + 写回）。

2560×1440 一帧 370 万像素，本机实测 **≈42 ms**。60fps 的视频光是这一步就要占满
两个半核，H.264 "硬解接近零 CPU" 的好处在这里被吃掉了。

## 改了什么

`src/decode.rs`：`Yuv420Frame::to_rgba` 重写，新增私有的 `ChromaTerms`、`spread`、
`narrow`、`saturate`。

- **色度项一行算一次。** 上游的变换是 `(256·y + k) >> 8`，其中 `k` 只依赖色度。
  因为 `256·y` 是 256 的整数倍，`(256·y + k) >> 8 == y + (k >> 8)` 恒成立（算术右移是
  向下取整，整数倍可以直接提出来）。所以每个色度行把 `k >> 8` 三路（红、绿、蓝）算好、
  铺成与画面等宽的三行 `i16`，这一个色度行管的两行亮度都拿它用。
- **亮度行是一次逐元素映射。** `像素 = [饱和(y + 红), 饱和(y + 绿), 饱和(y + 蓝), 255]`，
  输出按 `as_chunks_mut::<4>` 切，和亮度、三行色度项 zip 起来走；没有下标运算、没有
  边界检查，编译器能自动向量化。
- **输出一次分配到位**（`vec![0; w*h*4]`），不再逐像素 `extend`。平面数据比声明的尺寸短
  时（上游是遇到缺行就停），这里同样停在那一行，再把输出截到已转换的行数 —— 长度与上游
  一致。
- 宽度为奇数时最后一列只有半个色度对，`spread` 单独处理尾巴。

没有 `unsafe`，没有 `std::arch`：各目标自己拿各自的向量宽度。

## 逐位等于上游

不是"误差在容忍范围内"。上游的逐像素实现原样留在 `#[cfg(test)]` 里当对照（`per_pixel`，
它走的是没动过的 `crate::avc444::yuv_to_rgba`）：

| 测试 | 守的是什么 |
| --- | --- |
| `to_rgba_matches_the_per_pixel_transform` | 奇数 / 偶数宽高、带 padding 的 stride、1×1 这类边角尺寸，逐字节相等 |
| `to_rgba_matches_the_per_pixel_transform_for_every_chroma_pair` | U、V 各 256 个值的全部 65536 种组合，配上能让通道落在 0..255 中间的亮度（否则低位算错会被饱和钳位盖住） |
| `to_rgba_of_an_empty_picture_is_empty` | 宽或高为 0 不分配、不越界 |
| `to_rgba_cost_against_the_per_pixel_transform`（`#[ignore]`） | 量尺：同一进程里新旧交替跑、取中位数，顺带再断言一次逐字节相等 |

## 收益

Windows 11 x86_64，release，2560×1440 一帧，进程内新旧交替取中位数，跑了 5 遍：

| | 每帧 |
| --- | --- |
| 上游逐像素 | 40.9 – 43.9 ms |
| 本副本 | 4.7 – 5.05 ms |

**×8.3 – 9.4。** 这是纯色彩转换这一步的数；H.264 解码本身在 Media Foundation 里，
不在这个量尺内。没有连真实服务器量端到端的 CPU —— 那一项要人工验。

## 没动的

- `Yuv444Frame::to_rgba`（AVC444）：同样是逐像素写法，但 4:4:4 没有"一个色度样本管四个
  像素"的便宜可占，收益小；服务器协商到 AVC444 的情形也少。先不动。
- `ironrdp-egfx-native` 里 NV12 → 三平面的拆分（逐像素 push）：在另一个 crate 里，
  为它再 vendor 一份不值当。

## 什么时候可以删掉这份副本

上游有同一方向的 PR：Devolutions/IronRDP#2216 "perf(egfx): convert YUV 4:2:0 pictures
a pixel pair at a time"，写这段时（2026-11-25）还开着、没合。它合并并随新版本发布后，
把根 `Cargo.toml` 里 `ironrdp-egfx = { path = "vendor/ironrdp-egfx" }` 那一行和本目录
一起删掉，升版本即可。

## 验证

单独跑这份副本时把构建产物指到仓库的 `target/` 下，别落在 `vendor/` 里：

```sh
CARGO_TARGET_DIR=target/vendor-egfx cargo test --manifest-path vendor/ironrdp-egfx/Cargo.toml --release
# 70 项通过，1 项 ignored（量尺）

CARGO_TARGET_DIR=target/vendor-egfx cargo test --manifest-path vendor/ironrdp-egfx/Cargo.toml --release \
    to_rgba_cost -- --ignored --nocapture
# 量尺：打印新旧两边的中位数

cargo test --release -p winssh-rdp -p winssh-desktop -- --test-threads=1
# 应用侧接的是这份副本（`cargo tree -p winssh-rdp -i ironrdp-egfx` 显示 path）
```
