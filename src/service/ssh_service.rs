//! SSH 连接服务：UI 层打开会话的唯一入口。
//!
//! 按 AGENTS.md 的分层规矩，界面不直接依赖 `russh`，只通过这里拿连接。
//! 数据（`Host`）来自 `HostService`，连接与认证由 `crate::ssh` 完成，
//! 会话状态（`HostStatus` / `SessionTelemetry`）后续会由此处回填。

use anyhow::Result;

use crate::{
    host::Host,
    ssh::{SshConnection, SshSession, WindowSize},
};

/// SSH 连接的业务入口。无状态，廉价克隆。
#[derive(Clone, Default, Debug)]
pub struct SshService;

impl SshService {
    pub fn new() -> Self {
        Self
    }

    /// 按 [`Host`] 建立一条已认证的交互式 Shell 通道。
    ///
    /// 失败时把对端的提示原样带回，调用方（[`crate::ui::panels::TerminalPanel`]）
    /// 决定是弹通知还是在终端里展示错误。
    ///
    /// `russh` 基于 `tokio`，必须在 `tokio` 的 reactor 上执行；界面侧的
    /// `gpui` executor 没有 reactor，直接 `await` 会 panic（`there is no
    /// reactor running`）。因此这里经由 [`crate::db::run`] 派到应用级
    /// `tokio` runtime 上，`await` 到的仍是普通 future，GPUI 侧可直接等。
    pub async fn connect(&self, host: &Host, window: WindowSize) -> Result<SshConnection> {
        let host = host.clone();
        crate::db::run(async move { SshConnection::connect(&host, window).await }).await
    }

    /// 在一条已经建立好的连接上再开一条交互式 Shell。
    ///
    /// 分屏用这条：第二个窗格要的是"同一台机器上的另一个 shell"，不是
    /// 另一次登录。不重新认证也就没有第二次密码提示，开出来是立刻的。
    pub async fn open_shell(
        &self,
        session: SshSession,
        window: WindowSize,
    ) -> Result<SshConnection> {
        crate::db::run(async move { SshConnection::open_shell(session, window).await }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{AuthMethod, Host};

    fn unreachable_host() -> Host {
        Host {
            id: "test-unreachable".into(),
            name: "unreachable".into(),
            user: "root".into(),
            hostname: "127.0.0.1".into(),
            port: 22222,
            platform: "Linux".into(),
            auth_method: AuthMethod::Password,
            password: "wrong".into(),
            key_path: "".into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
        }
    }

    #[test]
    fn service_connect_without_tokio_reactor_does_not_panic() {
        // 模拟 GPUI 线程：没有 tokio reactor，直接 block_on 一个非 tokio executor。
        // 修复前这里会 panic `there is no reactor running`。
        let host = unreachable_host();
        let window = WindowSize::new(80, 24, 640, 384);
        let service = SshService::new();
        let result = futures::executor::block_on(service.connect(&host, window));
        assert!(result.is_err(), "不可达主机应返回错误而非 panic");
    }
}
