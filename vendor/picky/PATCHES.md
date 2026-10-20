# 这份 `picky` 与上游的差别

`vendor/picky` 逐字来自 crates.io 的 **picky 7.0.0-rc.25**
（<https://github.com/Devolutions/picky-rs>，MIT OR Apache-2.0，两份 LICENSE 与
`.cargo_vcs_info.json` 都原样留着），`Cargo.toml` 里的 `[patch.crates-io]`
指着它。改动两处：**16 条精确锁定的 RustCrypto 预发布版本放到对应的稳定版**，
外加一行用不上的导入。

## 为什么要动它

picky 7.0.0-rc.25 把一整批 RustCrypto crate 精确锁在预发布版上
（`=5.0.0-rc.1`、`=0.14.0-rc.14`、`=3.0.0-rc.1` …），而本应用的另一条链 ——
russh 的 SSH 密钥 —— 用的是同一批 crate 的**稳定版**（curve25519-dalek 5.0.0、
p256/p384/p521 0.14.0、ed25519-dalek 3.0.0…）。预发布版与稳定版属于同一个
semver 兼容段，Cargo 的解析器对同一段只允许选中一份，于是图根本解不开：

    error: failed to select a version for `curve25519-dalek`
      ... required by package `picky v7.0.0-rc.25`
      ... versions that meet the requirements `=5.0.0-rc.1` are: 5.0.0-rc.1
      ... previously selected package `curve25519-dalek v5.0.0`
          ... required by package `russh v0.63.3`

picky 不是可选项：`sspi`（CredSSP/NLA）与 `ironrdp-connector` 都要它，
`picky::key` 甚至不在 feature 门控后面。而 RustCrypto 那边 rc 到 stable 的
API 是照着走的（这批 crate 本就是同一批代码的晋级），把这些 `=` 去掉之后
picky 编译通过（`cargo check -p picky` 干净）。三处 pin 的稳定版还不存在
（`rsa` 0.10.0-rc.18、`pkcs1` 0.8.0-rc.4、`rustcrypto-ff/group/ff_derive`
0.14.0-rc.x），它们没有稳定版可放，就把 `=` 去掉、保留预发布版本号 ——
反正 russh 那条链不用它们，图上只有 picky/sspi 这一份。

关掉 `jose`（`ironrdp-connector` 的补丁做了这件事）之后，被放宽的
`aes-gcm`/`argon2`/`blake2` 这几条根本不会被激活，改它们只是让图上不再出现
两个 aes-gcm 候选。

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `Cargo.toml` | 16 条 `version = "=x.y.z-rc.N"` 改为稳定版（能放的）或去掉 `=`（没有稳定版的三个 RustCrypto 基础 crate）；注释说明见文件内。`Cargo.toml.orig` 是上游原文，保持不动。 |
| `src/signature.rs` | 删掉顶层第 8 行的 `SignatureEncoding as _`；Ed25519 签名分支里的 `raw_sign(...).to_vec()` 改为 `.to_bytes().to_vec()`。`Signature::to_vec()` 是 `ed25519/alloc` 特性下的固有方法 —— 全 workspace 构建时 `ssh-key` 把这个特性打开，顶层 trait 导入会报 `unused_imports`；而单独构建某个窄 crate 时该特性没开，`to_vec()` 又得靠 `SignatureEncoding` 这个 trait 才编得过。改用无条件的固有 `to_bytes()` 后两种构建都不再依赖特性组合。 |

其余文件（`src/` 其它、LICENSE、CHANGELOG、README、测试数据）与 crates.io 上
的 7.0.0-rc.25 逐字节相同。

注意：本项目里 picky 只以**无 feature** 的形态被编译（sspi 与
`vendor/ironrdp-connector` 都写了 `default-features = false`），所以
`jose` / `pkcs12` / `ssh` / `putty` 这些模块的代码不参与构建 ——
放宽 pin 的风险面也因此小得多。

## 想改成 `refs/` 那样的子模块

与 `vendor/vnc-rs/PATCHES.md` 末节的说明相同：把这份目录推成一个仓库、
`[patch.crates-io]` 改指 `{ git = "…", rev = "…" }` 即可。

## 改回上游

上游把这批精确锁换成稳定版（或 semver 范围）之后，删掉本目录与
`Cargo.toml` 里对应的 `[patch.crates-io]` 一行即可；若那时 picky 的
`jose` 仍是 `ironrdp-connector` 的 default，还要一并把
`vendor/ironrdp-connector` 的补丁撤掉。