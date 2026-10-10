    use super::*;

    /// 一条挂在假 sshd 上的会话，名字随便起。
    async fn target(name: &str) -> (SessionTarget, crate::ssh::exec::fake::Seen) {
        let (handle, seen) = crate::ssh::exec::fake::connect("total 0\n", "", true).await;
        (
            SessionTarget {
                id: name.into(),
                name: name.into(),
                login: "root@10.0.0.1".into(),
                platform: "Ubuntu".into(),
                handle,
            },
            seen,
        )
    }

    /// 工具端到端：命令要一字不差地进远端 `/bin/sh` 的标准输入，输出与
    /// 退出码收回来，同时给界面报一条 `Command` 事件。用的是进程内那台
    /// 假 sshd（[`crate::ssh::exec::fake`]），不碰网络。
    #[test]
    fn run_command_reaches_the_remote_shell_and_reports_back() {
        crate::db::runtime().block_on(async {
            let (target, seen) = target("web-01").await;
            let (tx, mut rx) = mpsc::unbounded();
            let session = CurrentSession::default();
            session.set(Some(target));
            let tool = RunCommand {
                session,
                events: tx,
            };

            let payload = tool
                .call(
                    &mut ToolContext::new(),
                    RunCommandArgs {
                        command: "ls -la /tmp".into(),
                        timeout_secs: None,
                    },
                )
                .await
                .unwrap();

            assert_eq!(
                seen.command(),
                "/bin/sh",
                "命令要交给 /bin/sh 而非登录 shell"
            );
            assert_eq!(seen.script(), "ls -la /tmp", "脚本一字不差进 stdin");
            assert_eq!(payload["stdout"], "total 0\n");
            assert_eq!(payload["exit_status"], 0);

            let Some(AgentEvent::Command {
                session,
                command,
                output,
                ok,
            }) = rx.next().await
            else {
                panic!("该有一条 Command 事件");
            };
            assert_eq!(session, "web-01");
            assert_eq!(command, "ls -la /tmp");
            assert_eq!(output, "total 0\n");
            assert!(ok);
        });
    }

    /// 眼下没有会话在前面时不该 panic 也不该抛错：模型要在 `error` 字段里
    /// 读到"得先连一台"，才能自己纠偏。
    #[test]
    fn no_open_session_reports_why_instead_of_failing() {
        crate::db::runtime().block_on(async {
            let (tx, _rx) = mpsc::unbounded();
            let tool = RunCommand {
                session: CurrentSession::default(),
                events: tx,
            };

            let payload = tool
                .call(
                    &mut ToolContext::new(),
                    RunCommandArgs {
                        command: "uptime".into(),
                        timeout_secs: None,
                    },
                )
                .await
                .unwrap();
            let error = payload["error"].as_str().unwrap();
            assert!(error.contains("NOT executed"), "错误里要说清没跑：{error}");
        });
    }

    /// 换标签页就是换机器：同一件工具，当前会话换一条，命令就落到另一台上。
    ///
    /// 这是"只操作当前打开的会话"的正面证据 —— 工具没有选机器的参数，它认
    /// 的就是执行那一刻槽里的那一条。
    #[test]
    fn the_tool_follows_whatever_session_is_current() {
        crate::db::runtime().block_on(async {
            let (web, web_seen) = target("web-01").await;
            let (db, db_seen) = target("db-02").await;
            let (tx, mut rx) = mpsc::unbounded();
            let session = CurrentSession::default();
            session.set(Some(web));
            let tool = RunCommand {
                session: session.clone(),
                events: tx,
            };

            let mut run = async |command: &str| {
                tool.call(
                    &mut ToolContext::new(),
                    RunCommandArgs {
                        command: command.to_owned(),
                        timeout_secs: None,
                    },
                )
                .await
                .unwrap();
                let Some(AgentEvent::Command { session, .. }) = rx.next().await else {
                    panic!("该有一条 Command 事件");
                };
                session
            };

            assert_eq!(run("uptime").await, "web-01");
            // 用户切了标签页。
            session.set(Some(db));
            assert_eq!(run("df -h").await, "db-02", "切过标签页之后该落在新机器上");

            assert_eq!(web_seen.script(), "uptime", "第一条该进 web-01");
            assert_eq!(db_seen.script(), "df -h", "第二条该进 db-02");
        });
    }

    /// 系统提示词里写的是当前那一条会话；没有会话时明说没有。
    ///
    /// 它每一轮都重写（[`Gate::on_completion_call`]），所以切标签页之后模型
    /// 下一句话看到的就是新的那一台。
    #[test]
    fn the_preamble_names_the_current_session() {
        crate::db::runtime().block_on(async {
            let (web, _) = target("web-01").await;
            let text = preamble(Some(&web));
            assert!(text.contains("web-01"), "提示词里该写着当前会话：{text}");
            assert!(!text.contains("{session}"), "占位符没替掉");
            assert!(preamble(None).contains(NO_SESSION));
        });
    }

    /// 审核门：批准放行、拒绝换成 Skip、全自动直接放行、按了停止就 Stop，
    /// 没有会话时连问都不问。
    #[test]
    fn the_gate_translates_decisions_into_actions() {
        crate::db::runtime().block_on(async {
            const ARGS: &str = r#"{"command":"rm -rf /tmp/x"}"#;

            let (web, _) = target("web-01").await;
            let current = || {
                let session = CurrentSession::default();
                session.set(Some(web.clone()));
                session
            };

            // 没有会话可操作：不发审核事件，直接回一句给模型。
            let (tx, mut rx) = mpsc::unbounded();
            let gate = Gate {
                events: tx,
                session: CurrentSession::default(),
                auto: false,
                cancel: Arc::new(AtomicBool::new(false)),
            };
            assert!(matches!(gate.review(ARGS).await, ToolCallAction::Skip(_)));
            assert!(rx.try_recv().is_err(), "没有会话时不该惊动用户");

            // 全自动：不送审，直接放行。
            let (tx, mut rx) = mpsc::unbounded();
            let gate = Gate {
                events: tx,
                session: current(),
                auto: true,
                cancel: Arc::new(AtomicBool::new(false)),
            };
            assert!(matches!(gate.review(ARGS).await, ToolCallAction::Run));
            assert!(rx.try_recv().is_err(), "全自动模式不该发审核事件");

            // 手动档：批准 → Run，拒绝 → Skip。审核事件里带着命令原文，
            // 目标写的是当前会话 —— 模型本来就没得选。
            for (approve, want_run) in [(true, true), (false, false)] {
                let (tx, mut rx) = mpsc::unbounded();
                let gate = Gate {
                    events: tx,
                    session: current(),
                    auto: false,
                    cancel: Arc::new(AtomicBool::new(false)),
                };
                let decide = async {
                    let Some(AgentEvent::Approval(request)) = rx.next().await else {
                        panic!("该有一条审核事件");
                    };
                    assert_eq!(request.command, "rm -rf /tmp/x");
                    assert_eq!(request.session, "web-01");
                    _ = request.respond.send(approve);
                };
                let (action, ()) = futures::join!(gate.review(ARGS), decide);
                match action {
                    ToolCallAction::Run => assert!(want_run, "拒绝之后不该放行"),
                    ToolCallAction::Skip(_) => assert!(!want_run, "批准之后不该跳过"),
                    other => panic!("不该是 {other:?}"),
                }
            }

            // 卡片还挂着的时候用户换了台机器：批准不作数，那条命令不能落到
            // 卡片上没写过的机器上。
            let (db, _) = target("db-02").await;
            let (tx, mut rx) = mpsc::unbounded();
            let session = current();
            let gate = Gate {
                events: tx,
                session: session.clone(),
                auto: false,
                cancel: Arc::new(AtomicBool::new(false)),
            };
            let decide = async {
                let Some(AgentEvent::Approval(request)) = rx.next().await else {
                    panic!("该有一条审核事件");
                };
                session.set(Some(db));
                _ = request.respond.send(true);
            };
            let (action, ()) = futures::join!(gate.review(ARGS), decide);
            assert!(
                matches!(action, ToolCallAction::Skip(_)),
                "换了机器之后那次批准不该还算数"
            );

            // 停止按钮压倒一切。
            let (tx, _rx) = mpsc::unbounded();
            let gate = Gate {
                events: tx,
                session: current(),
                auto: true,
                cancel: Arc::new(AtomicBool::new(true)),
            };
            assert!(matches!(gate.review(ARGS).await, ToolCallAction::Stop(_)));
        });
    }

    /// 一张规整过的小图。
    fn image() -> ChatImage {
        let mut png = Vec::new();
        ::image::DynamicImage::ImageRgb8(::image::RgbImage::new(4, 4))
            .write_to(
                &mut std::io::Cursor::new(&mut png),
                ::image::ImageFormat::Png,
            )
            .unwrap();
        ChatImage::prepare(&png).unwrap()
    }

    fn user(text: &str, images: usize) -> Turn {
        Turn {
            role: ChatRole::User,
            text: text.into(),
            images: vec![image(); images],
        }
    }

    fn images_in(message: &Message) -> usize {
        match message {
            Message::User { content } => content
                .iter()
                .filter(|part| matches!(part, UserContent::Image(_)))
                .count(),
            _ => 0,
        }
    }

    fn texts_in(message: &Message) -> Vec<String> {
        match message {
            Message::User { content } => content
                .iter()
                .filter_map(|part| match part {
                    UserContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// 图怎么进消息：提问自己的全带；历史里从最近的往前数，带满上限为止，
    /// 更早的换成一句说明；图排在文字前面，内联的是 PNG 的 base64。
    #[test]
    fn images_ride_along_up_to_the_history_limit() {
        let history = vec![
            user("第一轮", 3),
            Turn {
                role: ChatRole::Assistant,
                text: "看到了".into(),
                images: Vec::new(),
            },
            user("第二轮", 3),
            user("第三轮", 3),
        ];
        let (messages, prompt) = to_messages(history, user("现在呢", 2), true);

        assert_eq!(images_in(&prompt), 2, "这一次的图一张都不能少");
        assert_eq!(
            messages.iter().map(images_in).collect::<Vec<_>>(),
            [0, 0, 3, 3],
            "历史里只带最近的 {HISTORY_IMAGE_LIMIT} 张"
        );
        let oldest = texts_in(&messages[0]);
        assert!(
            oldest[0].contains("3 image(s)") && oldest[0].contains("no longer included"),
            "没带上的地方该留一句说明：{oldest:?}"
        );
        assert_eq!(oldest.last().map(String::as_str), Some("第一轮"));

        let Message::User { content } = &prompt else {
            panic!("提问该是一条用户消息");
        };
        assert!(
            matches!(content.first(), Some(UserContent::Image(_))),
            "图在前"
        );
        assert!(
            matches!(content.last(), Some(UserContent::Text(_))),
            "字在后"
        );
        let Some(UserContent::Image(inlined)) = content.first() else {
            unreachable!()
        };
        assert_eq!(inlined.media_type, Some(ImageMediaType::PNG));
        assert_eq!(
            inlined.data,
            rig_core::completion::message::DocumentSourceKind::Base64(image().to_base64())
        );
    }

    /// 模型没开看图：一张都不发（哪怕是这一次提问的），每条换成一句说明。
    #[test]
    fn a_text_only_model_gets_notes_instead_of_images() {
        let (messages, prompt) = to_messages(vec![user("之前那张", 2)], user("现在这张", 1), false);
        assert_eq!(images_in(&messages[0]), 0);
        assert_eq!(images_in(&prompt), 0);
        assert!(texts_in(&messages[0])[0].contains("2 image(s)"));
        assert!(texts_in(&prompt)[0].contains("without image input"));
        assert_eq!(texts_in(&prompt)[1], "现在这张");
    }

    /// 只贴了图、一个字没写：消息里只有图，没有空的文字块（Anthropic 不收）。
    #[test]
    fn an_image_only_prompt_has_no_empty_text_block() {
        let (_, prompt) = to_messages(Vec::new(), user("  ", 1), true);
        let Message::User { content } = &prompt else {
            panic!("提问该是一条用户消息");
        };
        assert_eq!(content.len(), 1);
        assert!(matches!(content[0], UserContent::Image(_)));
    }
