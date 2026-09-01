//! 在一条已建立的 SSH 连接上跑一次性命令并把输出收回来。
//!
//! 交互式 Shell 那条通道归终端（[`crate::terminal::Emulator`]），采集不能借用它
//! —— 往用户正在敲的命令行里插一条 `cat /proc/stat` 是不可接受的。SSH 本身是
//! 多路复用的：同一条连接上再开一条 `session` 通道、`exec` 一条命令、读完就关，
//! 与终端那条互不干扰。这也是 `handle` 要用 [`std::sync::Arc`] 共享的原因。
//!
//! 脚本不作为 `exec` 的命令行送出，而是喂给远端的 `/bin/sh` 的标准输入：
//! `exec` 的字符串由**登录 shell** 解释，而登录 shell 可能是 fish 或 csh，
//! `$(...)`、`2>/dev/null` 这些 POSIX 写法在那里语法不同。`/bin/sh` 是一个
//! 裸命令，任何 shell 都能启动它，脚本本身则由 `sh` 按 POSIX 规则读。

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use russh::{ChannelMsg, client::Handle};

use super::client::SshHandler;

/// 一次采集最多收多少字节。远端命令跑飞了（比如 `##ps` 段接了个刷屏的命令）
/// 也不能把内存吃光；正常一次采集不到 64 KiB。
const MAX_OUTPUT: usize = 1 << 20;

/// 一条一次性命令的执行结果。
#[derive(Clone, Debug, Default)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    /// 远端进程的退出码。对端不报的时候是 `None`。
    pub exit_status: Option<u32>,
}

impl ExecOutput {
    /// 退出码为 0（或对端没报）才算成功。
    pub fn success(&self) -> bool {
        self.exit_status.is_none_or(|status| status == 0)
    }

    /// 给用户看的失败原因：优先用 stderr 的第一行，没有就退回退出码。
    pub fn failure_reason(&self) -> String {
        let line = self
            .stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty());
        match (line, self.exit_status) {
            (Some(line), _) => line.to_owned(),
            (None, Some(status)) => format!("远端命令退出码 {status}"),
            (None, None) => "远端命令没有输出也没有退出码".to_owned(),
        }
    }
}

/// 把一段 POSIX sh 脚本送到远端执行，收集 stdout / stderr 与退出码。
///
/// `timeout` 到点就放弃这一次采集：对端卡住（磁盘 hang、负载爆表）时不能让
/// 界面的轮询任务一直挂着。
pub async fn exec_script(
    handle: &Handle<SshHandler>,
    script: &str,
    timeout: Duration,
) -> Result<ExecOutput> {
    match tokio::time::timeout(timeout, run_script(handle, script)).await {
        Ok(result) => result,
        Err(_) => Err(anyhow!("远端命令超过 {} 秒未返回", timeout.as_secs())),
    }
}

async fn run_script(handle: &Handle<SshHandler>, script: &str) -> Result<ExecOutput> {
    let mut channel = handle
        .channel_open_session()
        .await
        .context("打开采集通道失败")?;

    channel
        .exec(true, "/bin/sh")
        .await
        .context("远端启动 /bin/sh 失败")?;
    channel
        .data(script.as_bytes())
        .await
        .context("发送采集脚本失败")?;
    // sh 读到 EOF 才开始收尾；不发这一下，通道会一直等着更多输入。
    channel.eof().await.context("关闭采集通道的写端失败")?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_status = None;

    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => append(&mut stdout, &data),
            ChannelMsg::ExtendedData { data, .. } => append(&mut stderr, &data),
            ChannelMsg::ExitStatus {
                exit_status: status,
            } => exit_status = Some(status),
            ChannelMsg::Close => break,
            _ => {}
        }
    }

    Ok(ExecOutput {
        // 远端的输出理论上是 UTF-8（脚本自己钉了 `LC_ALL=C`），但进程名与
        // 命令行来自别人写的程序，坏字节不能让整次采集失败。
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_status,
    })
}

/// 追加一段输出，到上限就不再收 —— 已经收到的那部分仍然可用。
fn append(buffer: &mut Vec<u8>, data: &[u8]) {
    if buffer.len() >= MAX_OUTPUT {
        return;
    }
    let room = MAX_OUTPUT - buffer.len();
    buffer.extend_from_slice(&data[..data.len().min(room)]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_reason_prefers_stderr() {
        let output = ExecOutput {
            stdout: String::new(),
            stderr: "\n  sh: docker: not found\n".into(),
            exit_status: Some(127),
        };
        assert!(!output.success());
        assert_eq!(output.failure_reason(), "sh: docker: not found");
    }

    #[test]
    fn missing_exit_status_counts_as_success() {
        let output = ExecOutput::default();
        assert!(output.success());
    }
}

/// 一台只会做一件事的 sshd，跑在进程内的回环端口上。
///
/// 采集链条里只有这一段没法靠解析测试覆盖：通道开没开、脚本是不是送进了
/// `/bin/sh` 的标准输入、EOF 有没有发、stdout 与 stderr 分没分开、退出码收没收到。
/// `russh` 自带服务端实现，所以这些可以在进程内闭环验证，不需要网络，也不需要
/// 本机装 sshd。[`crate::service::MonitorService`] 的测试也用它，那边验的是
/// 「脚本 → 遥测」的整条路。
#[cfg(test)]
pub(crate) mod fake {
    use std::sync::{Arc, Mutex};

    use russh::keys::{Algorithm, PrivateKey};
    use russh::server::{self, Auth, Msg as ServerMsg, Server as _, Session};
    use russh::{Channel, ChannelId, client};
    use tokio::net::TcpListener;

    use super::*;

    /// 服务端看见了什么。测试拿它断言客户端送出去的东西。
    #[derive(Clone, Default)]
    pub(crate) struct Seen {
        /// 写进 stdin 的那段脚本。
        pub script: Arc<Mutex<Vec<u8>>>,
        /// `exec` 请求里的命令行。
        pub command: Arc<Mutex<Vec<u8>>>,
        /// `subsystem` 请求里要的名字。
        pub subsystem: Arc<Mutex<Vec<u8>>>,
    }

    impl Seen {
        pub(crate) fn script(&self) -> String {
            String::from_utf8_lossy(&self.script.lock().unwrap()).into_owned()
        }

        pub(crate) fn command(&self) -> String {
            String::from_utf8_lossy(&self.command.lock().unwrap()).into_owned()
        }

        pub(crate) fn subsystem(&self) -> String {
            String::from_utf8_lossy(&self.subsystem.lock().unwrap()).into_owned()
        }
    }

    #[derive(Clone)]
    struct Sshd {
        stdout: &'static str,
        stderr: &'static str,
        /// 假的对端答不答话。`false` 用来测超时。
        answer: bool,
        seen: Seen,
    }

    impl server::Server for Sshd {
        type Handler = Self;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
            self.clone()
        }
    }

    impl server::Handler for Sshd {
        type Error = russh::Error;

        async fn auth_password(&mut self, _: &str, _: &str) -> Result<Auth, Self::Error> {
            Ok(Auth::Accept)
        }

        async fn channel_open_session(
            &mut self,
            _: Channel<ServerMsg>,
            reply: server::ChannelOpenHandle,
            _: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: ChannelId,
            command: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.seen.command.lock().unwrap().extend_from_slice(command);
            session.channel_success(channel)?;
            Ok(())
        }

        /// 这台假的不提供 SFTP 子系统 —— 正好是真实环境里 `sshd_config` 没配
        /// `Subsystem sftp` 时客户端看到的那一句 `CHANNEL_FAILURE`。
        async fn subsystem_request(
            &mut self,
            channel: ChannelId,
            name: &str,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.seen
                .subsystem
                .lock()
                .unwrap()
                .extend_from_slice(name.as_bytes());
            session.channel_failure(channel)?;
            Ok(())
        }

        async fn data(
            &mut self,
            _: ChannelId,
            data: &[u8],
            _: &mut Session,
        ) -> Result<(), Self::Error> {
            self.seen.script.lock().unwrap().extend_from_slice(data);
            Ok(())
        }

        /// 客户端说"脚本写完了"。真 sh 这时候才开始跑，这里直接给答案。
        async fn channel_eof(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            if !self.answer {
                return Ok(());
            }
            session.data(channel, self.stdout.as_bytes().to_vec())?;
            if !self.stderr.is_empty() {
                session.extended_data(channel, 1, self.stderr.as_bytes().to_vec())?;
            }
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
            Ok(())
        }
    }

    /// 起一台假 sshd 并连上去，返回可以直接交给 [`exec_script`] 的句柄。
    ///
    /// 必须在 tokio runtime 上调用（`crate::db::runtime().block_on`）。
    pub(crate) async fn connect(
        stdout: &'static str,
        stderr: &'static str,
        answer: bool,
    ) -> (Arc<Handle<SshHandler>>, Seen) {
        let seen = Seen::default();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定回环端口");
        let port = listener.local_addr().unwrap().port();
        let config = Arc::new(server::Config {
            keys: vec![
                PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("生成临时主机密钥"),
            ],
            ..Default::default()
        });
        let mut sshd = Sshd {
            stdout,
            stderr,
            answer,
            seen: seen.clone(),
        };
        tokio::spawn(async move { sshd.run_on_socket(config, &listener).await });

        let mut handle = client::connect(
            Arc::new(client::Config::default()),
            ("127.0.0.1", port),
            SshHandler,
        )
        .await
        .expect("连上回环 sshd");
        assert!(
            handle
                .authenticate_password("tester".to_owned(), "hunter2".to_owned())
                .await
                .expect("认证请求")
                .success()
        );
        (Arc::new(handle), seen)
    }

    const REPLY: &str = "##hz
100 4
##end
";
    const WARNING: &str = "sh: docker: not found
";

    #[test]
    fn script_goes_to_stdin_and_output_comes_back_split() {
        crate::db::runtime().block_on(async {
            let (handle, seen) = connect(REPLY, WARNING, true).await;

            let script = "echo '##hz'
";
            let output = exec_script(&handle, script, Duration::from_secs(10))
                .await
                .expect("采集应成功");

            assert_eq!(output.stdout, REPLY, "stdout 原样收回");
            assert_eq!(output.stderr, WARNING, "stderr 单独收，不混进 stdout");
            assert_eq!(output.exit_status, Some(0));
            assert!(output.success());
            assert_eq!(
                seen.command(),
                "/bin/sh",
                "脚本要交给 /bin/sh，不能让登录 shell 去解释"
            );
            assert_eq!(seen.script(), script, "脚本要一字不差地进 stdin");
        });
    }

    /// 对端不吭声时不能一直挂着 —— 轮询任务会卡在这里，面板就永远停在
    /// "正在采集"。
    #[test]
    fn a_silent_server_times_out() {
        crate::db::runtime().block_on(async {
            let (handle, _) = connect("", "", false).await;
            let error = exec_script(
                &handle,
                "sleep 1
",
                Duration::from_millis(300),
            )
            .await
            .expect_err("超时必须报错");
            assert!(format!("{error}").contains("未返回"), "实际：{error}");
        });
    }
}
