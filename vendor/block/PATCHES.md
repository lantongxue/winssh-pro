# 这份 `block` 与上游的差别

`vendor/block` 逐字来自 crates.io 的 **block 0.1.6**（<https://github.com/SSheldon/rust-block>，
MIT），`Cargo.toml` 里的 `[patch.crates-io]` 指着它。

为什么要动它：macOS 那一档的 GPUI 平台层经 `cocoa` / `cocoa-foundation` /
`core-video` / `core-graphics2` / `metal` 把它拖进来，而它把
`extern static _NSConcreteStackBlock` 声明成空 enum（不可居留类型），每次构建都报

```
warning: the following packages contain code that will be rejected by a future version of Rust: block v0.1.6
```

即 `uninhabited_static` 这条 future-incompat lint（rust-lang/rust#74840），将来会变成
硬错误。0.1.6 是 crates.io 上的最后一版（上游已转去 `block2`），上面那几个依赖也没换，
没有"升级即修"的路。

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `src/lib.rs` | `enum Class { }` → `#[repr(C)] struct Class { _priv: [u8; 0] }`。零尺寸不透明类型，代码里只取 `&_NSConcreteStackBlock` 的地址，从不读值，ABI 与行为都不变。 |
| `src/lib.rs` | 所有 `extern fn` / `extern { … }` 补上显式的 `"C"`。省略 ABI 时默认就是 `"C"`，行为不变；只为消掉 `missing_abi` —— 换成 path 依赖后 cargo 不再对它 `--cap-lints`，这 46 条会出现在每次构建里。 |

依赖它的 crate 什么时候换成 `block2`（或上游发了修好的版本），这份补丁连同
`[patch.crates-io]` 里那一行一起删掉即可。
