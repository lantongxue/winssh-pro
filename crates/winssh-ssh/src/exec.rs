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
use tokio_util::sync::CancellationToken;

use super::client::SshHandler;
use rust_i18n::t;

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
            (None, Some(status)) => t!("ssh_exec.exit_status", status => status).to_string(),
            (None, None) => t!("ssh_exec.no_output").to_string(),
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
    match tokio::time::timeout(timeout, run_script(handle, script, None)).await {
        Ok(result) => result,
        Err(_) => Err(anyhow!(
            "{}",
            t!("ssh_exec.timeout", seconds => timeout.as_secs())
        )),
    }
}

/// 与 [`exec_script`] 相同，但能被 `cancel` 提前打断：取消时关闭通道 ——
/// 远端命令收到挂断，不会在后台接着跑完。取消本身按错误返回，调用方按自己的
/// 取消语义处理（传输层会把它落成"已取消"，不是一条给用户看的失败）。
///
/// 服务端复制（`cp`）用它：一次大目录的复制可能跑很久，面板关掉或用户点取消
/// 时得真把远端那条命令停下来。
pub async fn exec_script_cancellable(
    handle: &Handle<SshHandler>,
    script: &str,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<ExecOutput> {
    match tokio::time::timeout(timeout, run_script(handle, script, Some(cancel))).await {
        Ok(result) => result,
        Err(_) => Err(anyhow!(
            "{}",
            t!("ssh_exec.timeout", seconds => timeout.as_secs())
        )),
    }
}

/// 在同一条连接上开一条**长活**的 `exec` 通道，把它的双向字节流交出去。
///
/// 与 [`exec_script`] 那条"跑完收输出就关"不同：这条把通道交给调用方持续读写，
/// 通道活到调用方把它丢掉为止。Docker 的 `docker system dial-stdio` 走它
/// （见 `winssh-docker`）—— 一条 HTTP 连接对应一条这样的通道，SSH 的多路复用
/// 让若干条 HTTP 连接与终端互不干扰。
///
/// 命令本身的成败由调用方在流上自己读：`exec` 请求只是发出去，远端那一句
/// `docker: command not found` 是随后从 stderr 来的，这里不收。要干净地分辨
/// "没装 / 没权限"，调用方先跑一次 [`exec_script`] 探一下（`DockerService` 这么做）。
pub async fn exec_stream(
    handle: &Handle<SshHandler>,
    command: &str,
) -> Result<russh::ChannelStream<russh::client::Msg>> {
    let channel = handle
        .channel_open_session()
        .await
        .with_context(|| t!("ssh_exec.open_channel_failed").to_string())?;
    channel
        .exec(true, command)
        .await
        .with_context(|| t!("ssh_exec.exec_failed").to_string())?;
    Ok(channel.into_stream())
}

async fn run_script(
    handle: &Handle<SshHandler>,
    script: &str,
    cancel: Option<&CancellationToken>,
) -> Result<ExecOutput> {
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return Err(canceled());
    }

    let mut channel = handle
        .channel_open_session()
        .await
        .with_context(|| t!("ssh_exec.open_channel_failed").to_string())?;

    channel
        .exec(true, "/bin/sh")
        .await
        .with_context(|| t!("ssh_exec.start_sh_failed").to_string())?;
    channel
        .data(script.as_bytes())
        .await
        .with_context(|| t!("ssh_exec.send_script_failed").to_string())?;
    // sh 读到 EOF 才开始收尾；不发这一下，通道会一直等着更多输入。
    channel
        .eof()
        .await
        .with_context(|| t!("ssh_exec.close_write_failed").to_string())?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_status = None;

    loop {
        let message = match cancel {
            // `biased`：取消优先于这一轮收到的数据检查，点下取消就能立刻收手。
            Some(cancel) => {
                let mut aborted = false;
                let message = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        aborted = true;
                        None
                    }
                    message = channel.wait() => message,
                };
                if aborted {
                    // 关通道就是挂断远端命令；不关的话它会继续把整个目录写完。
                    let _ = channel.close().await;
                    return Err(canceled());
                }
                message
            }
            None => channel.wait().await,
        };
        let Some(message) = message else {
            break;
        };
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

/// 取消时返回的错误。文案只是给日志看的，传输层知道这是取消，不会当失败弹。
fn canceled() -> anyhow::Error {
    anyhow!("{}", t!("ssh_exec.canceled"))
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
#[path = "tests/ssh/exec.rs"]
mod tests;

/// 一台只会做一件事的 sshd，跑在进程内的回环端口上。
///
/// 采集链条里只有这一段没法靠解析测试覆盖：通道开没开、脚本是不是送进了
/// `/bin/sh` 的标准输入、EOF 有没有发、stdout 与 stderr 分没分开、退出码收没收到。
/// `russh` 自带服务端实现，所以这些可以在进程内闭环验证，不需要网络，也不需要
/// 本机装 sshd。[`crate::service::MonitorService`] 的测试也用它，那边验的是
/// 「脚本 → 遥测」的整条路。
#[cfg(any(test, feature = "test-support"))]
pub mod fake {
    use std::sync::{Arc, Mutex};

    use russh::keys::{Algorithm, PrivateKey};
    use russh::server::{self, Auth, Msg as ServerMsg, Server as _, Session};
    use russh::{Channel, ChannelId, client};
    use tokio::net::TcpListener;

    use super::*;

    /// 服务端看见了什么。测试拿它断言客户端送出去的东西。
    #[derive(Clone, Default)]
    pub struct Seen {
        /// 写进 stdin 的那段脚本。
        pub script: Arc<Mutex<Vec<u8>>>,
        /// `exec` 请求里的命令行。
        pub command: Arc<Mutex<Vec<u8>>>,
        /// `subsystem` 请求里要的名字。
        pub subsystem: Arc<Mutex<Vec<u8>>>,
    }

    impl Seen {
        pub fn script(&self) -> String {
            String::from_utf8_lossy(&self.script.lock().unwrap()).into_owned()
        }

        pub fn command(&self) -> String {
            String::from_utf8_lossy(&self.command.lock().unwrap()).into_owned()
        }

        pub fn subsystem(&self) -> String {
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
    /// 必须在 tokio runtime 上调用（`winssh_db::runtime().block_on`）。
    pub async fn connect(
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
            SshHandler::new(
                Default::default(),
                ("127.0.0.1".into(), port),
                winssh_domain::trusted_host::TrustPolicy::Skip,
            ),
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

    // 只给本 crate 自己那个 `#[test]` 用（`#[test]` 仅在 `cfg(test)` 下编）；
    // 当本 crate 以 `test-support` 作为依赖被编时（供上层测试用 `connect`/`Seen`），
    // 这两个常量没人用，故同样用 `cfg(test)` 门掉，不留 dead_code 警告。
    #[cfg(test)]
    const REPLY: &str = "##hz
100 4
##end
";
    #[cfg(test)]
    const WARNING: &str = "sh: docker: not found
";

    #[test]
    fn script_goes_to_stdin_and_output_comes_back_split() {
        winssh_db::runtime().block_on(async {
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

    /// 取消要真把那次执行收掉，不能等对端开口。服务端复制关面板时走这条路。
    #[test]
    fn canceling_stops_the_script_run() {
        winssh_db::runtime().block_on(async {
            let (handle, _) = connect("", "", false).await;
            let cancel = CancellationToken::new();
            cancel.cancel();
            let error = exec_script_cancellable(
                &handle,
                "sleep 100
",
                Duration::from_secs(10),
                &cancel,
            )
            .await
            .expect_err("取消必须报错");
            assert_eq!(format!("{error}"), t!("ssh_exec.canceled"));
        });
    }

    /// 对端不吭声时不能一直挂着 —— 轮询任务会卡在这里，面板就永远停在
    /// "正在采集"。
    #[test]
    fn a_silent_server_times_out() {
        winssh_db::runtime().block_on(async {
            let (handle, _) = connect("", "", false).await;
            let error = exec_script(
                &handle,
                "sleep 1
",
                Duration::from_millis(300),
            )
            .await
            .expect_err("超时必须报错");
            assert_eq!(
                format!("{error}"),
                t!("ssh_exec.timeout", seconds => 0),
                "实际：{error}"
            );
        });
    }
}
