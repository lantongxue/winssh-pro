//! SFTP 子系统：在同一条 SSH 连接上再开一条通道，把它交给 SFTP 协议。
//!
//! 与终端（[`crate::terminal::Emulator`]）、监控采集（[`super::exec`]）并列，是这条
//! 连接的第三类用法 —— SSH 本来就是多路复用的，`handle` 因此是
//! [`std::sync::Arc`] 共享的。开法与 `exec` 同构：`session` 通道 →
//! `subsystem sftp`，区别是这条通道要长期留着讲二进制协议，所以
//! [`russh::Channel::into_stream`] 之后交给 [`russh_sftp::client::SftpSession`]，
//! 由它自己在 Tokio 上跑读写两半。
//!
//! [`russh_sftp`] 只讲协议、不碰 socket（russh 于它只是 dev-dependency），
//! 所以它和我们的 russh 0.63 之间没有版本关系；喂进去的字节流才是连接本身。
//!
//! 远端没装 `sftp-server` 或对端禁了子系统时，`request_subsystem` 会被拒绝 ——
//! 这里把原因说清楚，因为界面能做的只有"告诉你没有 SFTP"。

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use russh::{Channel, ChannelMsg, client::Handle};
use russh_sftp::client::SftpSession;

use super::client::SshHandler;
use rust_i18n::t;

/// 在已认证的连接上开一条 SFTP 会话。
///
/// 返回 [`Arc`]：会话本身的方法都只要 `&self`（内部已是 `Arc<RawSftpSession>` +
/// 通道），但服务层与传输任务要各自持有一份，引用计数是它们共享的唯一途径。
/// 最后一个句柄丢掉时通道关闭，远端子系统随之退出。
pub async fn open_session(handle: &Handle<SshHandler>) -> Result<Arc<SftpSession>> {
    let mut channel = handle
        .channel_open_session()
        .await
        .with_context(|| t!("ssh_sftp.open_channel_failed").to_string())?;

    channel
        .request_subsystem(true, "sftp")
        .await
        .with_context(|| t!("ssh_sftp.request_subsystem_failed").to_string())?;

    // `request_subsystem(true, ...)` 只是把请求发出去，`want_reply` 不是"在这儿等"
    // 的意思 —— 回音是通道上的 `Success`/`Failure`，得自己读。不读的话，被拒绝时
    // 界面看到的只会是握手超时的"协议握手失败"，而真原因是"这台机器没给你 SFTP"。
    await_subsystem_reply(&mut channel).await?;

    let session = SftpSession::new(channel.into_stream())
        .await
        .with_context(|| t!("ssh_sftp.handshake_failed").to_string())?;

    Ok(Arc::new(session))
}

/// 等这一条通道上的子系统回音，只认 `Success`。
async fn await_subsystem_reply(channel: &mut Channel<russh::client::Msg>) -> Result<()> {
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Success => return Ok(()),
            ChannelMsg::Failure => bail!("{}", t!("ssh_sftp.subsystem_refused")),
            ChannelMsg::Close => bail!("{}", t!("ssh_sftp.closed_early")),
            // 数据不该出现在这里（子系统还没起来），收到了就丢掉继续等。
            other => log::trace!("sftp subsystem 等待回音时收到：{other:?}"),
        }
    }
    bail!("{}", t!("ssh_sftp.disconnected"))
}

#[cfg(test)]
#[path = "../tests/ssh/sftp.rs"]
mod tests;
