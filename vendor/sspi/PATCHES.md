# 这份 `sspi` 与上游的差别

`vendor/sspi` 逐字来自 crates.io 的 **sspi 0.21.3**
（<https://github.com/Devolutions/sspi-rs>，MIT OR Apache-2.0，两份 LICENSE 与
`.cargo_vcs_info.json` 都原样留着），`Cargo.toml` 里的 `[patch.crates-io]`
指着它。改动只有一处：**三条精确锁定的 RustCrypto 预发布版本放到对应的
稳定版**（curve25519-dalek `=5.0.0-rc.1` → `5.0.0`、ed25519-dalek
`=3.0.0-rc.1` → `3.0.0`、p256/p384/p521/primeorder `=0.14.0-rc.14` →
`0.14.0`）。

## 为什么要动它

与 `vendor/picky/PATCHES.md` 是同一场冲突、同一个理由：russh 的密钥链要这批
crate 的稳定版，sspi（`ironrdp-connector` 的 CredSSP/NLA 实现）把它们锁在
预发布版上，同一个 semver 兼容段 Cargo 只能选一份，图解不开。picky 那份补丁
先把 picky 自己的锁打开了，sspi 这份跟着打开 sspi 自己的 —— 两处 pin 的
来源不同（各 crate 各锁各的），所以必须各改一份。

`rsa`、`pkcs1`、`rustcrypto-ff/group/ff_derive` 这几条没有稳定版可换，保留
`=` 精确锁；russh 那条链不用它们。

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `Cargo.toml` | 六条 `version = "=x.y.z-rc.N"` 改为对应稳定版。`Cargo.toml.orig` 是上游原文，保持不动。 |

其余文件与 crates.io 上的 0.21.3 逐字节相同。本项目只走
`sspi::credssp`（NTLM）这一条路，被放宽的 curve25519/ed25519/p 系列曲线
只在 Kerberos/智能卡相关路径上；它们与 russh 用的是同一份稳定版实现。

## 想改成 `refs/` 那样的子模块

与 `vendor/vnc-rs/PATCHES.md` 末节的说明相同。

## 改回上游

上游把这批精确锁换成稳定版之后，删掉本目录与 `Cargo.toml` 里对应的
`[patch.crates-io]` 一行即可。若上游同时把 picky 的 pin 也放开了，
`vendor/picky` 那份补丁也可以一并撤掉。