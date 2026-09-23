# 这份 `ironrdp-connector` 与上游的差别

`vendor/ironrdp-connector` 逐字来自 crates.io 的 **ironrdp-connector 0.10.0**
（<https://github.com/Devolutions/IronRDP>，MIT OR Apache-2.0，两份 LICENSE 与
`.cargo_vcs_info.json` 都原样留着），`Cargo.toml` 里的 `[patch.crates-io]`
指着它。改动只有一处，一行：**把 `picky` 的 default features 关掉**。

## 为什么要动它

ironrdp-connector 的 `picky` 依赖没写 `default-features = false`，于是吃到了
picky 的出厂特性集（`x509, jose, http_signature, http_trait_impl, pkcs12`），
其中 `jose` 把 `aes-gcm` 钉在 `=0.11.0-rc.4` 上。而本应用另一条链条 ——
russh 的 SSH 私钥解密 —— 走的是

    russh → ssh-key/encryption → cipher/aes → ssh-cipher → aes-gcm "0.11"

也就是 aes-gcm **稳定版**（0.11.1）。rc 与稳定版同属 0.11 兼容段，Cargo 的
解析器对同一个兼容段只允许选中一份，于是整个图解不开：

    error: failed to select a version for `aes-gcm`
      ... required by package `picky v7.0.0-rc.25`
      ... versions that meet the requirements `^0.11` are: 0.11.1, 0.11.0
      ... previously selected package `aes-gcm v0.11.0-rc.4`

两边都绕不开：russh 的 ssh-key 依赖把 `encryption` 写死在自己的 Cargo.toml 里，
关不掉；ironrdp-connector 的 picky 版本是精确锁定 `=7.0.0-rc.25`，换不掉。
在这份本地副本上关掉 default features 是唯一一处能落刀的地方 —— 而且它恰好
**不改变任何被用到的功能**：本 crate 全文只用到

    use picky::key::PrivateKey;   // src/credssp.rs:3

而 `picky::key` 不在任何 feature 门控后面（`src/lib.rs` 里 `pub mod key;`
裸着）。关掉 default 之后，这份 picky 的编译配置与本项目图里 sspi 早已在用的
那一份**完全相同**（sspi 的 Cargo.toml 同样写着 `default-features = false`），
也就是说这份补丁只是把 ironrdp-connector 对齐到 sspi 的用法，并没有引入任何
新的 picky 构建组合。

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `Cargo.toml` | `[dependencies.picky]` 下加一行 `default-features = false`（外加一段说明注释）。`Cargo.toml.orig` 是上游原文，保持不动以便对照。 |

其余文件（`src/` 全部、LICENSE、CHANGELOG、README）与 crates.io 上的 0.10.0
逐字节相同。

## 想改成 `refs/` 那样的子模块

`refs/` 下是浅克隆的子模块，而这里放不下一个子模块 —— 上游没有这个改动。
要挪过去：

1. 把这份目录推成一个仓库（自己 fork 一份 `Devolutions/IronRDP`，把这一行提上去）；
2. 删掉 `vendor/ironrdp-connector`，`git submodule add <你的 fork> refs/ironrdp`；
3. `Cargo.toml` 的 `[patch.crates-io]` 改指 `{ git = "…", rev = "…" }`。

同样要注意 `AGENTS.md` 那条"禁止修改 `refs/` 下的代码"，`vendor/vnc-rs/PATCHES.md`
末尾对这件事的提醒在这里一样适用。

## 改回上游

上游把这个依赖写成 `default-features = false` 之后，删掉本目录与
`Cargo.toml` 里对应的 `[patch.crates-io]` 一行即可，应用侧零改动。