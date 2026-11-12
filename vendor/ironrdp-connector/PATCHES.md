# 这份 `ironrdp-connector` 与上游的差别

`vendor/ironrdp-connector` 逐字来自 crates.io 的 **ironrdp-connector 0.12.0**
（<https://github.com/Devolutions/IronRDP>，MIT OR Apache-2.0，两份 LICENSE 与
`.cargo_vcs_info.json` 都原样留着），`Cargo.toml` 里的 `[patch.crates-io]`
指着它。改动一处：`src/connection.rs` 里**删掉一条已经失效的
`#[expect(single_use_lifetimes)]`**。

## 历史：0.10 时代还有第二处补丁（已不需要）

0.10.0 的 `picky` 依赖没写 `default-features = false`，出厂特性集里的
`jose` 把 `aes-gcm` 钉在 `=0.11.0-rc.4`，与 russh 密钥链要的稳定版
（0.11.1）同属 0.11 兼容段，Cargo 解不开，当时在这份副本上关掉了
picky 的 default features。**0.12.0 钉的 picky 7.0.0-rc.26 已把 aes-gcm
放宽到 `^0.11`**，与 russh 共用稳定版即可，那处补丁随之退役。同理，
`vendor/picky`、`vendor/sspi` 两份旧批次的 RustCrypto 钉版补丁也不再需要
（新批次上游自己放宽了冲突的锁），`[patch.crates-io]` 不再指它们。

## `src/connection.rs` 的那条 `#[expect]`

`create_gcc_blocks<'a>` 的唯一一处 `&'a` 写在 `impl Iterator<Item = &'a …>` 里面。
`impl Trait` 里的匿名生命周期至今不稳定（`&'_` 与不写都是 E0658），所以那个
`<'a>` 是删不掉的；但 rustc 已经不再对"生命周期只出现在 `impl Trait` 约束里"这一
种写法报 `single_use_lifetimes`（1.98.0 实测，见下方复现），上游钉在原地的
`#[expect(single_use_lifetimes)]` 就反过来变成了 `unfulfilled_lint_expectations`
警告 —— 编译这个 crate 每次刷一行。

于是删掉那条 expect，只在原地留一段注释，说明这里为什么必须有名生命周期。没换成
`#[allow(single_use_lifetimes)]`：allow 是把话头按到底（哪天 rustc 又改回去报这条
lint，这里是静默的），而本 crate 自己 `[lints.clippy]` 里的 `allow_attributes = "warn"`
又不鼓励 `#[allow]` 这种写法。

复现（不需要整个依赖树）：

```sh
rustc --edition 2024 -W single_use_lifetimes --crate-type lib probe.rs
# probe.rs 里只留 `fn f<'a>(_i: impl Iterator<Item = &'a str>) {}` —— 无任何警告
```

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `src/connection.rs` | 删掉 `create_gcc_blocks` 上面那条 `#[expect(single_use_lifetimes)]`，换成一段说明注释。函数签名与函数体一字未动。 |

其余文件（`Cargo.toml`、`src/` 的其余部分、LICENSE、CHANGELOG、README）与
crates.io 上的 0.12.0 逐字节相同。

## 想改成 `refs/` 那样的子模块

`refs/` 下是浅克隆的子模块，而这里放不下一个子模块 —— 上游没有这个改动。
要挪过去：

1. 把这份目录推成一个仓库（自己 fork 一份 `Devolutions/IronRDP`，把这一行提上去）；
2. 删掉 `vendor/ironrdp-connector`，`git submodule add <你的 fork> refs/ironrdp`；
3. `Cargo.toml` 的 `[patch.crates-io]` 改指 `{ git = "…", rev = "…" }`。

同样要注意 `AGENTS.md` 那条"禁止修改 `refs/` 下的代码"，`vendor/vnc-rs/PATCHES.md`
末尾对这件事的提醒在这里一样适用。

## 改回上游

上游把 `create_gcc_blocks` 上那条 `#[expect(single_use_lifetimes)]` 删掉
（或者上游把 rust-toolchain 提到不再报这条 expect 的版本）之后，删掉本目录
与 `Cargo.toml` 里对应的 `[patch.crates-io]` 一行即可，应用侧零改动。