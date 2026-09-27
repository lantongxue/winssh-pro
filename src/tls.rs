//! 进程级的 rustls 加密后端。
//!
//! 依赖图上的 rustls 同时开着两个后端：`aws-lc-rs`（russh、IronRDP、reqwest 一直
//! 用的那一份）和 `ring`（EasyTier 的 WebSocket / QUIC 隧道点亮的）。两个都在时
//! rustls 没法替谁挑，`ClientConfig::builder()` 这类不指名后端的入口会当场
//! panic（"no process-level CryptoProvider available"）—— 而 `ironrdp-tls`、
//! `hyper-rustls`、EasyTier 自己的 wss 隧道与节点发现走的都是这个入口。
//!
//! 所以开进程时先把 `aws-lc-rs` 装成默认：和 EasyTier 进来之前每一条 TLS 用的
//! 是同一个后端，行为一点没变；显式指名 `ring` 的那几处（quinn、tokio-websockets）
//! 照旧用它们自己点的那一个。
//!
//! 还有一层非装不可的理由：EasyTier 的 wss 隧道第一次用的时候会自己去装默认
//! 后端（`install_default(ring)`，装不上就算了）。我们不抢先装的话，谁先连就
//! 定了谁 —— 先开一张走 wss 的网，之后 RDP、reqwest 的 TLS 就悄悄换成了 ring。
//! 在 `main` 的第一行装好，这件事就不看先后了。

/// 把 `aws-lc-rs` 装成进程默认的 rustls 后端。幂等：已经装过（包括别人先装了
/// 一个）就什么也不做 —— 进程里只能有一个默认后端，谁先装算谁的。
///
/// `main` 开窗前调一次；组网会话开之前也调一次（[`crate::easytier`]），
/// 测试里不经 `main` 也能起 wss 隧道。
pub fn install_default_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}
