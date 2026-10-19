    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
    use crate::exec::fake;

    /// 对端不接子系统时，错误里要有那句人话，而不是一个裸的 `Request denied`。
    ///
    /// 假的 sshd（见 [`fake`]）不提供 SFTP 子系统，直接回 `CHANNEL_FAILURE` ——
    /// 真实环境里 `sshd_config` 没配 `Subsystem sftp` 就是这样。
    #[test]
    fn a_server_without_sftp_says_so() {
        winssh_db::runtime().block_on(async {
            let (handle, seen) = fake::connect("", "", true).await;

            let result = tokio::time::timeout(
                Duration::from_secs(10),
                open_session(&handle, &SftpLaunch::Subsystem),
            )
            .await;

            let error = match result {
                Ok(Ok(_)) => panic!("没有子系统的连接不该返回会话"),
                Ok(Err(error)) => error,
                Err(_) => panic!("对端已经答过话，不该一直等下去"),
            };

            let message = format!("{error:#}");
            assert!(
                message.contains(t!("ssh_sftp.subsystem_refused").as_ref()),
                "错误应说明是子系统的问题，实际：{message}"
            );
            assert_eq!(seen.subsystem(), "sftp", "请求的子系统名字要送对");
        });
    }

    // ─── 纯逻辑：命令行与握手状态机 ─────────────────────────────────────────

    #[test]
    fn sudo_command_is_one_single_quoted_line() {
        let command = sudo_command(None, "abc");
        // 外层整段单引号：登录 shell 可能是 fish / csh，里面再出现单引号、换行或
        // `!`（csh 的历史展开）都会被它们先改写一遍。
        assert!(command.starts_with("/bin/sh -c '") && command.ends_with('\''));
        let body = &command["/bin/sh -c '".len()..command.len() - 1];
        assert!(!body.contains('\''), "脚本里不能再有单引号：{body}");
        assert!(!body.contains('\n') && !body.contains('!'));
        assert!(body.contains("sudo -S -p WINSSH-SUDO-abc: -- "));
        assert!(body.contains("printf WINSSH-READY-abc"));
        assert!(body.contains("/usr/lib/openssh/sftp-server"));
        assert!(body.contains("/usr/libexec/openssh/sftp-server"));
        // 中文环境的服务器上 PAM 问的是「密码：」，sudo 就不换成我们的提示串，握手会
        // 干等到超时 —— 所以先钉英文环境，而且要在 sudo 之前。
        let locale = body.find("LC_ALL=C; export LC_ALL;").expect("要钉 LC_ALL=C");
        assert!(locale < body.find("sudo -S").unwrap());
    }

    #[test]
    fn sudo_command_switches_user_only_when_asked() {
        assert!(!sudo_command(None, "n").contains(" -u "));
        assert!(sudo_command(Some("www-data"), "n").contains(" -p WINSSH-SUDO-n: -u www-data -- "));
    }

    #[test]
    fn markers_split_across_packets_are_still_found() {
        let mut handshake = SudoHandshake::new("n1");
        assert_eq!(
            handshake.stderr(b"We trust you have received the usual lecture\nWINSSH-SU"),
            Step::Wait
        );
        assert_eq!(handshake.stderr(b"DO-n1:"), Step::Password { retry: false });
        // rc 文件在 stdout 前面垫了字，就绪串还被拆成两半。
        assert_eq!(handshake.stdout(b"motd junk\nWINSSH-REA"), Step::Wait);
        assert_eq!(handshake.stdout(b"DY-n1"), Step::Ready);
    }

    #[test]
    fn a_second_prompt_means_the_password_was_wrong() {
        let mut handshake = SudoHandshake::new("n");
        assert_eq!(
            handshake.stderr(b"WINSSH-SUDO-n:"),
            Step::Password { retry: false }
        );
        assert_eq!(handshake.stderr(b"Sorry, try again.\n"), Step::Wait);
        assert_eq!(
            handshake.stderr(b"WINSSH-SUDO-n:"),
            Step::Password { retry: true }
        );
    }

    #[test]
    fn a_marker_from_another_nonce_is_ignored() {
        // 随机串防的就是这个：别处的输出里碰巧有一段长得像的东西，不能当成
        // sudo 在要密码而把密码喂出去。
        let mut handshake = SudoHandshake::new("mine");
        assert_eq!(handshake.stderr(b"WINSSH-SUDO-theirs:"), Step::Wait);
        assert_eq!(handshake.stdout(b"WINSSH-READY-theirs"), Step::Wait);
    }

    #[test]
    fn failure_prefers_sudos_own_last_line() {
        let mut handshake = SudoHandshake::new("n");
        handshake.stderr(b"WINSSH-SUDO-n:Sorry, try again.\nWINSSH-SUDO-n:");
        handshake.stderr(b"sudo: 3 incorrect password attempts\n");
        let reason = handshake.failure(Some(1));
        assert!(
            reason.contains("sudo: 3 incorrect password attempts"),
            "{reason}"
        );
        assert!(
            !reason.contains("WINSSH-SUDO"),
            "提示串不该漏给用户：{reason}"
        );
    }

    #[test]
    fn missing_sftp_server_is_explained() {
        let mut handshake = SudoHandshake::new("n");
        handshake.stderr(b"WINSSH-NO-SFTP-SERVER\n");
        assert_eq!(handshake.failure(Some(127)), t!("ssh_sftp.sudo_no_server"));
    }

    #[test]
    fn password_line_ends_with_exactly_one_newline() {
        assert_eq!(password_line("hunter2"), "hunter2\n");
        // 从剪贴板粘进来的常带一个回车换行；留着的话 sudo 读到的是半截。
        assert_eq!(password_line("hunter2\r\n"), "hunter2\n");
    }

    #[test]
    fn identity_names_the_switch() {
        assert_eq!(SftpLaunch::Subsystem.identity(), None);
        let auth: Arc<dyn SudoAuth> = Arc::new(ScriptedAuth::new([]));
        let root = SftpLaunch::Sudo {
            user: None,
            auth: auth.clone(),
        };
        assert_eq!(root.identity().as_deref(), Some("sudo"));
        let deploy = SftpLaunch::Sudo {
            user: Some("deploy".into()),
            auth,
        };
        assert_eq!(deploy.identity().as_deref(), Some("sudo -u deploy"));
        assert!(deploy.is_elevated());
    }

    // ─── 闭环：一台会装 sudo 的假 sshd ─────────────────────────────────────

    /// 按剧本答密码的 [`SudoAuth`]，顺手记下被问了几次、收下了哪个。
    struct ScriptedAuth {
        answers: Mutex<VecDeque<Option<String>>>,
        asked: Mutex<Vec<bool>>,
        accepted: Mutex<Option<String>>,
    }

    impl ScriptedAuth {
        fn new<const N: usize>(answers: [Option<&str>; N]) -> Self {
            Self {
                answers: Mutex::new(answers.iter().map(|a| a.map(str::to_owned)).collect()),
                asked: Mutex::new(Vec::new()),
                accepted: Mutex::new(None),
            }
        }
    }

    impl SudoAuth for ScriptedAuth {
        fn password(&self, retry: bool) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> {
            self.asked.lock().unwrap().push(retry);
            let answer = self.answers.lock().unwrap().pop_front().flatten();
            Box::pin(async move { answer })
        }

        fn accepted(&self, password: &str) {
            *self.accepted.lock().unwrap() = Some(password.to_owned());
        }
    }

    mod sudo_sshd {
        use std::sync::{Arc, Mutex};

        use russh::keys::{Algorithm, PrivateKey};
        use russh::server::{self, Auth, Msg as ServerMsg, Server as _, Session};
        use russh::{Channel, ChannelId, client};
        use tokio::net::TcpListener;

        use crate::client::SshHandler;

        /// 假 sudo 收到的 stdin（密码那几行）。
        #[derive(Clone, Default)]
        pub struct Stdin(pub Arc<Mutex<Vec<u8>>>);

        impl Stdin {
            pub fn text(&self) -> String {
                String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
            }
        }

        /// 行为照着真 sudo 来：要密码就在 stderr 打 `-p` 给的提示串；错了说
        /// `Sorry, try again.` 再问，三次都错就退出 1；对了（或者根本不要密码）
        /// 就跑 `sh -c 'printf READY; exec sftp-server'` —— 这里只打就绪串。
        #[derive(Clone)]
        struct Sshd {
            /// `None` 是 NOPASSWD。
            password: Option<&'static str>,
            prompt: Arc<Mutex<String>>,
            ready: Arc<Mutex<String>>,
            line: Arc<Mutex<Vec<u8>>>,
            failures: Arc<Mutex<u32>>,
            stdin: Stdin,
        }

        impl server::Server for Sshd {
            type Handler = Self;

            fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
                self.clone()
            }
        }

        /// 从命令行里抠出 `WINSSH-<kind>-<nonce>` 那个标记。
        fn marker(command: &str, kind: &str, end: char) -> String {
            let start = command.find(kind).expect("命令行里有标记");
            let rest = &command[start..];
            let len = rest.find(end).expect("标记有结尾");
            rest[..len].to_owned()
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
                let command = String::from_utf8_lossy(command);
                let prompt = format!("{}:", marker(&command, "WINSSH-SUDO-", ':'));
                let ready = marker(&command, "WINSSH-READY-", ';');
                session.channel_success(channel)?;
                match self.password {
                    Some(_) => session.extended_data(channel, 1, prompt.as_bytes().to_vec())?,
                    None => session.data(channel, ready.as_bytes().to_vec())?,
                }
                *self.prompt.lock().unwrap() = prompt;
                *self.ready.lock().unwrap() = ready;
                Ok(())
            }

            async fn data(
                &mut self,
                channel: ChannelId,
                data: &[u8],
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                self.stdin.0.lock().unwrap().extend_from_slice(data);
                let mut line = self.line.lock().unwrap();
                line.extend_from_slice(data);
                while let Some(end) = line.iter().position(|&byte| byte == b'\n') {
                    let typed: Vec<u8> = line.drain(..=end).collect();
                    let typed = String::from_utf8_lossy(&typed[..end]).into_owned();
                    if Some(typed.as_str()) == self.password {
                        let ready = self.ready.lock().unwrap().clone();
                        session.data(channel, ready.into_bytes())?;
                        continue;
                    }
                    let mut failures = self.failures.lock().unwrap();
                    *failures += 1;
                    if *failures < 3 {
                        let prompt = self.prompt.lock().unwrap().clone();
                        session.extended_data(
                            channel,
                            1,
                            format!("Sorry, try again.\n{prompt}").into_bytes(),
                        )?;
                    } else {
                        session.extended_data(
                            channel,
                            1,
                            b"sudo: 3 incorrect password attempts\n".to_vec(),
                        )?;
                        session.eof(channel)?;
                        session.exit_status_request(channel, 1)?;
                        session.close(channel)?;
                    }
                }
                Ok(())
            }
        }

        pub async fn connect(
            password: Option<&'static str>,
        ) -> (Arc<client::Handle<SshHandler>>, Stdin) {
            let stdin = Stdin::default();
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
                password,
                prompt: Default::default(),
                ready: Default::default(),
                line: Default::default(),
                failures: Default::default(),
                stdin: stdin.clone(),
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
            (Arc::new(handle), stdin)
        }
    }

    /// 在假 sudo 上跑一遍 [`start_with_sudo`]，10 秒还没完就算挂住。
    async fn run_sudo(password: Option<&'static str>, auth: &ScriptedAuth) -> (Result<()>, String) {
        let (handle, stdin) = sudo_sshd::connect(password).await;
        let mut channel = handle.channel_open_session().await.expect("开通道");
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            start_with_sudo(&mut channel, None, auth),
        )
        .await
        .expect("握手不该挂住");
        (result, stdin.text())
    }

    #[test]
    fn password_is_fed_only_after_the_prompt_and_cached_on_success() {
        winssh_db::runtime().block_on(async {
            let auth = ScriptedAuth::new([Some("right")]);
            let (result, stdin) = run_sudo(Some("right"), &auth).await;
            result.expect("密码对了就该就绪");
            assert_eq!(stdin, "right\n", "stdin 上只该有那一行密码");
            assert_eq!(*auth.asked.lock().unwrap(), vec![false]);
            assert_eq!(auth.accepted.lock().unwrap().as_deref(), Some("right"));
        });
    }

    #[test]
    fn nopasswd_sudo_never_asks_and_writes_nothing() {
        // NOPASSWD（或授权还没过期）时 sudo 不问。这时盲喂一行密码，sftp-server
        // 会把它当成协议包读走，整条会话就坏了 —— 所以只能见了提示串才喂。
        winssh_db::runtime().block_on(async {
            let auth = ScriptedAuth::new([Some("unused")]);
            let (result, stdin) = run_sudo(None, &auth).await;
            result.expect("不要密码时直接就绪");
            assert_eq!(stdin, "", "没人问就一个字节也不能写");
            assert!(auth.asked.lock().unwrap().is_empty());
            assert_eq!(*auth.accepted.lock().unwrap(), None);
        });
    }

    #[test]
    fn a_wrong_password_is_retried_on_the_same_channel() {
        winssh_db::runtime().block_on(async {
            let auth = ScriptedAuth::new([Some("wrong"), Some("right")]);
            let (result, stdin) = run_sudo(Some("right"), &auth).await;
            result.expect("第二次对了就该就绪");
            assert_eq!(stdin, "wrong\nright\n");
            assert_eq!(*auth.asked.lock().unwrap(), vec![false, true]);
            assert_eq!(auth.accepted.lock().unwrap().as_deref(), Some("right"));
        });
    }

    #[test]
    fn three_wrong_passwords_surface_sudos_message() {
        winssh_db::runtime().block_on(async {
            let auth = ScriptedAuth::new([Some("a"), Some("b"), Some("c")]);
            let (result, _) = run_sudo(Some("right"), &auth).await;
            let message = format!("{:#}", result.expect_err("三次都错要失败"));
            assert!(
                message.contains("3 incorrect password attempts"),
                "{message}"
            );
            assert_eq!(*auth.accepted.lock().unwrap(), None, "错的密码不能进缓存");
        });
    }

    #[test]
    fn cancelling_the_password_prompt_fails_cleanly() {
        winssh_db::runtime().block_on(async {
            let auth = ScriptedAuth::new([None]);
            let (result, stdin) = run_sudo(Some("right"), &auth).await;
            let message = format!("{:#}", result.expect_err("取消要失败"));
            assert_eq!(message, t!("ssh_sftp.sudo_canceled"));
            assert_eq!(stdin, "");
        });
    }
