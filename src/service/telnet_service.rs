//! Telnet 连接服务：UI 层打开一条 Telnet 会话的唯一入口。
//!
//! 与 [`crate::service::SshService`] 是同一个角色 —— 界面按 [`crate::host::Host`]
//! 说要连哪台机器，这一层把它翻译成一次实际的 `TelnetConnection` 建立，
//! 不让 russh / telnet 这类协议库渗进 `src/ui/`。
//!
//! 与 SSH 那份不同的是：这里没有"再开一条 shell"的 [`SshService::open_shell`]
//! 概念 —— Telnet 一条 socket 只能有一条会话，分屏要开的是"再拨一次"。
//! 因此这里只有 `connect` 一个方法。

use anyhow::Result;

use crate::{host::Host, ssh::WindowSize, telnet::TelnetConnection};

/// Telnet 连接的业务入口。无状态，廉价克隆。
#[derive(Clone, Default, Debug)]
pub struct TelnetService;

impl TelnetService {
    pub fn new() -> Self {
        Self
    }

    /// 按 [`Host`] 建立一条已经协商完毕的 Telnet 会话。
    ///
    /// 两步，都不在 gpui 线程上：[`crate::db::run`] 让代码进应用级 tokio
    /// runtime，先在那里异步拨号（[`TelnetConnection::dial`]：直连或经代理，
    /// 经组网的主机还要等那张网）；拨通之后的初始协商是**同步阻塞**的（telnet
    /// crate 不吃 tokio），再用 `spawn_blocking` 踢到 blocking 池
    /// （[`TelnetConnection::start_blocking`]），不占 worker。
    pub async fn connect(&self, host: &Host, window: WindowSize) -> Result<TelnetConnection> {
        let host = host.clone();
        crate::db::run(async move {
            let stream = TelnetConnection::dial(&host).await?;
            let connection = tokio::task::spawn_blocking(move || {
                TelnetConnection::start_blocking(stream, window)
            })
            .await?;
            Ok(connection)
        })
        .await
    }
}

#[cfg(test)]
#[path = "../tests/service/telnet_service.rs"]
mod tests;
