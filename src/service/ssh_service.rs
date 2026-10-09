//! SSH 连接服务：UI 层打开会话的唯一入口。
//!
//! 按 AGENTS.md 的分层规矩，界面不直接依赖 `russh`，只通过这里拿连接。
//! 数据（`Host`）来自 `HostService`，连接与认证由 `crate::ssh` 完成，
//! 会话状态（`HostStatus` / `SessionTelemetry`）后续会由此处回填。

use anyhow::Result;

use crate::{
    credential::KeyAlgorithm,
    host::Host,
    ssh::{
        GeneratedKey, KeyFacts, SshConnection, SshSession, WindowSize, decode_private_key,
        generate_key, inspect_private_key,
    },
    trusted_host::TrustPolicy,
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
    #[allow(dead_code)]
    pub async fn connect(&self, host: &Host, window: WindowSize) -> Result<SshConnection> {
        let host = host.clone();
        crate::db::run(async move { SshConnection::connect(&host, window).await }).await
    }

    /// 建一条带主机密钥信任检查的连接。
    ///
    /// `trust` 决定握手时 `check_server_key` 的处置，见 [`TrustPolicy`]。
    /// 典型用法：首次连接传 `CaptureAndAbort`，失败后检查 `capture` 槽位，
    /// 展示确认框让用户决定是否信任；已有记录传 `Verify`。
    pub async fn connect_trusted(
        &self,
        host: &Host,
        window: WindowSize,
        trust: TrustPolicy,
    ) -> Result<SshConnection> {
        let host = host.clone();
        crate::db::run(async move { SshConnection::connect_trusted(&host, window, trust).await })
            .await
    }

    /// 拨号 + 认证之后就停手，把已认证的会话凭据交出来。
    ///
    /// 独立 SFTP（[`crate::host::HostKind::Sftp`]）走这条路：那台服务器上根本没有
    /// shell 可开，面板要的只是能在上面开子系统通道的那条连接（见
    /// [`crate::ssh::SshConnection::connect_session_trusted`]）。
    pub async fn connect_session_trusted(
        &self,
        host: &Host,
        trust: TrustPolicy,
    ) -> Result<SshSession> {
        let host = host.clone();
        crate::db::run(async move { SshConnection::connect_session_trusted(&host, trust).await })
            .await
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

    /// 这份私钥文本连同这个密码，能不能真的解出一把私钥来。
    ///
    /// 主机表单在存之前问一次：粘错文件（`.pub` 是最常见的那一个）、少复制
    /// 半截、密码敲错，这三样在这一步就说得清楚；不问的话，用户要等到下一次
    /// 开会话才知道刚才存下去的那台主机根本连不上。
    ///
    /// 派到 `spawn_blocking` 上：加密私钥要过一遍 bcrypt-pbkdf，那是**故意**
    /// 设计得慢的，在 gpui 线程上直接解会看得见地卡一下。
    pub async fn check_private_key(&self, content: String, passphrase: String) -> Result<()> {
        crate::db::run(async move {
            tokio::task::spawn_blocking(move || decode_private_key(&content, &passphrase).map(drop))
                .await?
        })
        .await
    }

    /// 解开一份私钥，问它自己的指纹、公钥与算法。
    ///
    /// 保险箱在存一份粘进来的私钥之前走这条：既当了一次"这份私钥认不认得"的
    /// 校验（和 [`Self::check_private_key`] 同一条路），又把卡片上那行指纹
    /// 从钥匙本身算了出来 —— 那一行是要和对端 `authorized_keys` 对得上的。
    pub async fn inspect_private_key(
        &self,
        content: String,
        passphrase: String,
    ) -> Result<KeyFacts> {
        crate::db::run(async move {
            tokio::task::spawn_blocking(move || inspect_private_key(&content, &passphrase)).await?
        })
        .await
    }

    /// 生成一对新的 SSH 密钥，交给保险箱存起来。
    ///
    /// 和上面那条同一个理由派到 `spawn_blocking`，而且更硬：RSA 4096 要找两个
    /// 2048 位的素数，那是**秒级**的纯计算，在 gpui 线程上跑就是整个界面停住
    /// 好几秒。真正生成的那几行在 [`crate::ssh::keygen`]。
    pub async fn generate_key(
        &self,
        algorithm: KeyAlgorithm,
        comment: String,
        passphrase: String,
    ) -> Result<GeneratedKey> {
        crate::db::run(async move {
            tokio::task::spawn_blocking(move || generate_key(algorithm, &comment, &passphrase))
                .await?
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{AuthMethod, Host};

    fn unreachable_host() -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            kind: crate::host::HostKind::Ssh,
            id: "test-unreachable".into(),
            name: "unreachable".into(),
            user: "root".into(),
            hostname: "127.0.0.1".into(),
            port: 22222,
            platform: "Linux".into(),
            auth_method: AuthMethod::Password,
            password: "wrong".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            shell_integration: false,
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

    /// 主机表单在 gpui 线程上调这条（同样没有 tokio reactor），认得的私钥
    /// 说好、粘错的东西说不好。私钥本身怎么解由
    /// [`crate::ssh::decode_private_key`] 那边的测试盯着，这条只管这层壳。
    #[test]
    fn checking_a_key_works_off_the_tokio_reactor() {
        const PLAIN_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACCSYs7/Ou/C2pPI7ZxckzxEd+/ccwDwhRrBZ5LSX/1hMAAAAJiU1PCmlNTw
pgAAAAtzc2gtZWQyNTUxOQAAACCSYs7/Ou/C2pPI7ZxckzxEd+/ccwDwhRrBZ5LSX/1hMA
AAAEA8+fBsnVb31tspnDJupy7iZ0yI+tnQezazJTwOlyeX9ZJizv8678Lak8jtnFyTPER3
79xzAPCFGsFnktJf/WEwAAAAD3dpbnNzaC1wcm8tdGVzdAECAwQFBg==
-----END OPENSSH PRIVATE KEY-----
";
        let service = SshService::new();

        let good = futures::executor::block_on(
            service.check_private_key(PLAIN_KEY.to_string(), String::new()),
        );
        assert!(good.is_ok(), "这是一把好私钥：{:?}", good.err());

        let bad = futures::executor::block_on(
            service.check_private_key("не ключ".to_string(), String::new()),
        );
        assert!(bad.is_err(), "不是私钥的东西不该通过");
    }
}
