    use super::*;
    use futures::FutureExt as _;
    use gpui_kit::component::dock::{
        DockLayout, DockPlacement, DockSkin, PanelStyle, panel_handle,
    };

    /// 一张编码好的 PNG（渐变，免得像纯色块那样被压得过分小）。
    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        ::image::DynamicImage::ImageRgb8(::image::RgbImage::from_fn(width, height, |x, y| {
            ::image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        }))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            ::image::ImageFormat::Png,
        )
        .unwrap();
        bytes
    }

    fn sample_image() -> ChatImage {
        ChatImage::prepare(&png_bytes(160, 90)).unwrap()
    }

    fn sample_messages() -> Vec<ChatMessage> {
        vec![
            ChatMessage {
                id: "m1".into(),
                role: ChatRole::User,
                body: MessageBody::Text {
                    text: "看看 web-01 磁盘还剩多少，报错截图在这".into(),
                    images: vec![sample_image(), sample_image()],
                    tokens_per_sec: None,
                    model: None,
                },
            },
            // 只贴了图、没写字的提问：不画气泡，也没有复制键。
            ChatMessage {
                id: "m0".into(),
                role: ChatRole::User,
                body: MessageBody::Text {
                    text: "".into(),
                    images: vec![sample_image()],
                    tokens_per_sec: None,
                    model: None,
                },
            },
            ChatMessage {
                id: "m2".into(),
                role: ChatRole::Assistant,
                body: MessageBody::Command {
                    command: "df -h /".into(),
                    session: "web-01".into(),
                    output: "/dev/vda1  40G  12G  28G  30% /".into(),
                    status: CommandStatus::Ok,
                },
            },
            ChatMessage {
                id: "m3".into(),
                role: ChatRole::Assistant,
                body: MessageBody::Command {
                    command: "rm -rf /var/log/old".into(),
                    session: "web-01".into(),
                    output: "".into(),
                    status: CommandStatus::Denied,
                },
            },
            // 想完了的一段思考：收着，只剩一行标题。
            ChatMessage {
                id: "m5".into(),
                role: ChatRole::Assistant,
                body: MessageBody::Reasoning {
                    text: "先看 df，再看 inode".into(),
                },
            },
            ChatMessage {
                id: "m4".into(),
                role: ChatRole::Assistant,
                body: MessageBody::reply("根分区还有 **28G** 可用。", Some(42.5)).with_model("glm-4.5"),
            },
        ]
    }

    /// 四种消息画法齐上阵地画一整帧：气泡、带速度的 Markdown 回复、带图的
    /// 提问、收着的与正在想的思考、三种状态的命令卡片，外加标签条上那三样
    /// （对话菜单、警示牌、两颗按钮）和输入行上的图片排（规整完的、还在规整
    /// 的）。深 builder 链上的错只有真的画一遍才掉得出来。
    ///
    /// 自动执行与看图都开着画：[`Panel::title_suffix`] 那块警示牌只有这一档
    /// 才画，输入行上的模式键这一档是琥珀色那套；最后拨回手动确认再画一帧。
    #[gpui_kit::test]
    fn the_panel_draws_every_message_shape(cx: &mut gpui_kit::TestAppContext) {
        let _exclusive = assistant_config::exclusive_for_test();
        let mut config = assistant_config::single_model_for_test("test-model", true);
        config.auto_execute = true;
        // 认思考强度的模型：输入行上那颗强度键也画出来。
        config.providers[0].models[0].efforts =
            vec![ThinkingEffort::None, ThinkingEffort::High];
        assistant_config::set_global(config);

        let (harness, cx) = dock_harness(cx, sample_messages());
        harness.update(cx, |harness, cx| {
            harness.assistant.update(cx, |panel, cx| {
                panel.drafts = vec![
                    Draft {
                        id: "d1".into(),
                        image: Some(sample_image()),
                    },
                    Draft {
                        id: "d2".into(),
                        image: None,
                    },
                ];
                // 正在想的那一段：摊开着，标题上走流光。
                panel.reasoning_delta("磁盘满了的话", cx);
                cx.notify();
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        // 第二帧走的不是同一条路：虚拟列表的量测只有这一帧才读得到。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        harness.read_with(cx, |harness, cx| {
            let panel = harness.assistant.read(cx);
            assert_eq!(panel.messages.len(), 7);
            assert!(!panel.running);
        });

        assistant_config::set_global(assistant_config::AssistantConfig::default());
        harness.update(cx, |harness, cx| harness.assistant.update(cx, |_, cx| cx.notify()));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 审核卡片开着也要画得通，而且"拒绝"要落一张 Denied 卡片、
    /// 把 `false` 送回运行侧。
    #[gpui_kit::test]
    fn denying_an_approval_records_a_card(cx: &mut gpui_kit::TestAppContext) {
        let (harness, cx) = dock_harness(cx, Vec::new());
        let panel = harness.read_with(cx, |harness, _| harness.assistant.clone());

        let (respond, decision) = oneshot::channel();
        panel.update(cx, |panel, cx| {
            panel.running = true;
            panel.pending = Some(PendingApproval {
                session: "web-01".into(),
                proposal: Proposal::Command("systemctl restart nginx".into()),
                respond: Some(respond),
            });
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        panel.update(cx, |panel, cx| panel.decide(false, cx));
        panel.read_with(cx, |panel, _| {
            assert!(panel.pending.is_none(), "答复之后卡片该收走");
            let Some(ChatMessage {
                body:
                    MessageBody::Command {
                        status: CommandStatus::Denied,
                        command,
                        ..
                    },
                ..
            }) = panel.messages.last()
            else {
                panic!("拒绝该落一张 Denied 卡片");
            };
            assert_eq!(command.as_ref(), "systemctl restart nginx");
        });
        assert_eq!(
            decision.now_or_never().and_then(Result::ok),
            Some(false),
            "运行侧该收到 false"
        );
    }

    fn upload_request(overwrite: bool) -> crate::assistant::TransferRequest {
        crate::assistant::TransferRequest {
            direction: TransferDirection::Upload,
            local: "/home/kali/dist".into(),
            remote: "/opt/app".into(),
            overwrite,
        }
    }

    /// 传输的审核卡片也要画得通（含"将覆盖"那行），拒绝落一张 Denied 的
    /// 传输卡片，两端路径照审核卡片上写的。
    #[gpui_kit::test]
    fn denying_a_transfer_records_a_transfer_card(cx: &mut gpui_kit::TestAppContext) {
        let (harness, cx) = dock_harness(cx, Vec::new());
        let panel = harness.read_with(cx, |harness, _| harness.assistant.clone());

        let (respond, decision) = oneshot::channel();
        panel.update(cx, |panel, cx| {
            panel.running = true;
            panel.pending = Some(PendingApproval {
                session: "web-01".into(),
                proposal: Proposal::Transfer(upload_request(true)),
                respond: Some(respond),
            });
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        panel.update(cx, |panel, cx| panel.decide(false, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.read_with(cx, |panel, _| {
            let Some(ChatMessage {
                body: MessageBody::Transfer(card),
                ..
            }) = panel.messages.last()
            else {
                panic!("拒绝该落一张传输卡片");
            };
            assert_eq!(card.status, CommandStatus::Denied);
            assert_eq!(card.source.as_ref(), "/home/kali/dist");
            assert_eq!(card.destination.as_ref(), "/opt/app");
        });
        assert_eq!(decision.now_or_never().and_then(Result::ok), Some(false));
    }

    /// 一次传输在对话里只占**一行**：开始摆一张带进度条的卡片，进度只改数，
    /// 结束把同一行换成搬完的那张、交出去落库 —— 不是再追加一张。
    #[gpui_kit::test]
    fn a_transfer_card_lives_in_one_row_from_start_to_finish(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (harness, cx) = dock_harness(cx, Vec::new());
        let panel = harness.read_with(cx, |harness, _| harness.assistant.clone());
        let planned = TransferCard::planned("web-01".into(), &upload_request(false), CommandStatus::Ok);

        panel.update(cx, |panel, cx| {
            let saved = panel.transfer_update(
                TransferUpdate::Started {
                    id: 7,
                    card: planned.clone(),
                },
                cx,
            );
            assert!(saved.is_none(), "开始那张不落库");
            // 目录还没走完，总数是 0：进度条转着等。
            panel.transfer_update(
                TransferUpdate::Progress {
                    id: 7,
                    transferred: 0,
                    total: 0,
                },
                cx,
            );
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.update(cx, |panel, cx| {
            panel.transfer_update(
                TransferUpdate::Progress {
                    id: 7,
                    transferred: 512,
                    total: 2048,
                },
                cx,
            );
            assert_eq!(panel.transfers[&7].transferred, 512);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let done = TransferCard {
            destination: "/opt/app/dist".into(),
            files: 3,
            bytes: 2048,
            ..planned
        };
        panel.update(cx, |panel, cx| {
            let saved = panel.transfer_update(
                TransferUpdate::Finished {
                    id: 7,
                    card: done.clone(),
                },
                cx,
            );
            assert_eq!(saved, Some(MessageBody::Transfer(done.clone())), "结束那张该交出去落库");
            assert_eq!(panel.messages.len(), 1, "结束是换掉那一行，不是再追加一张");
            assert_eq!(panel.messages[0].body, MessageBody::Transfer(done));
            assert!(panel.transfers.is_empty());
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 运行侧没报结束就收摊了：那张卡片别一直写着"传输中"。
    #[gpui_kit::test]
    fn an_abandoned_transfer_is_marked_failed(cx: &mut gpui_kit::TestAppContext) {
        let (harness, cx) = dock_harness(cx, Vec::new());
        let panel = harness.read_with(cx, |harness, _| harness.assistant.clone());
        panel.update(cx, |panel, cx| {
            let card = TransferCard::planned("web-01".into(), &upload_request(false), CommandStatus::Ok);
            panel.transfer_update(TransferUpdate::Started { id: 1, card }, cx);
            panel.abandon_transfers(cx);
            assert!(panel.transfers.is_empty());
            let MessageBody::Transfer(card) = &panel.messages[0].body else {
                panic!("该是传输卡片");
            };
            assert_eq!(card.status, CommandStatus::Failed);
        });
    }

    /// 写本机文件的审核卡片画得通（带正文开头、覆盖与敏感两行提醒），拒绝
    /// 落一张命令卡片，来处写"本机"。
    #[gpui_kit::test]
    fn denying_a_local_write_records_a_card_from_this_computer(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (harness, cx) = dock_harness(cx, Vec::new());
        let panel = harness.read_with(cx, |harness, _| harness.assistant.clone());
        let review = LocalReview {
            action: LocalAction::Write {
                path: "/home/kali/.ssh/config".into(),
                content: (1..40).map(|n| format!("line {n}\n")).collect(),
                overwrite: true,
            },
            sensitive: true,
            replaces: true,
        };

        let (respond, decision) = oneshot::channel();
        panel.update(cx, |panel, cx| {
            panel.running = true;
            panel.pending = Some(PendingApproval {
                session: crate::assistant::local::local_label(),
                proposal: Proposal::Local(review.clone()),
                respond: Some(respond),
            });
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        panel.update(cx, |panel, cx| panel.decide(false, cx));
        panel.read_with(cx, |panel, _| {
            let Some(ChatMessage {
                body:
                    MessageBody::Command {
                        status: CommandStatus::Denied,
                        command,
                        session,
                        ..
                    },
                ..
            }) = panel.messages.last()
            else {
                panic!("拒绝该落一张 Denied 卡片");
            };
            assert_eq!(command.as_ref(), review.action.describe());
            assert_eq!(session, &crate::assistant::local::local_label());
        });
        assert_eq!(decision.now_or_never().and_then(Result::ok), Some(false));
    }

    /// 标签条上换对话那颗按钮得真的画在屏幕上。
    ///
    /// 栽过一次：顺手实现了 [`Panel::tab_name`]，而 `render_tabs` 一看它有值
    /// 就整个换掉标题元素画一个纯字符串 —— 菜单连画都不画，换对话的唯一入口
    /// 当场没了，而 `cargo check` 和别的测试一个字都不会说。所以这条不问面板
    /// "你打算画什么"，它画完一整帧，去调试边界表里问"你画了没有"。
    #[gpui_kit::test]
    fn the_tab_bar_keeps_the_conversation_menu(cx: &mut gpui_kit::TestAppContext) {
        let (_harness, cx) = dock_harness(cx, sample_messages());
        assert!(
            cx.debug_bounds(CONVERSATION_MENU_SELECTOR).is_some(),
            "标签条上没有换对话的入口了"
        );
    }

    /// 流式输出：一轮里的增量接在同一条消息上，说完了整条换成规范全文，
    /// 下一轮另起一条。
    ///
    /// 那个"换"字是要紧的：增量在 rig 那边算暂定的（一轮可以被重试作废），
    /// 而且文字与工具调用的先后是收尾那一步才定的。要是把收尾那份接在增量
    /// 后面，用户会看到整段话说两遍 —— 而且库里存下的正是那两遍。
    #[gpui_kit::test]
    fn streaming_appends_within_a_turn_and_replaces_at_the_end(cx: &mut gpui_kit::TestAppContext) {
        fn text_of(message: &ChatMessage) -> &str {
            match &message.body {
                MessageBody::Text { text, .. } => text.as_ref(),
                MessageBody::Reasoning { .. }
                | MessageBody::Command { .. }
                | MessageBody::Transfer(_) => panic!("这条该是文字消息"),
            }
        }

        let (harness, cx) = dock_harness(cx, Vec::new());
        let panel = harness.read_with(cx, |harness, _| harness.assistant.clone());

        panel.update(cx, |panel, cx| {
            panel.stream_delta("根分区", cx);
            panel.stream_delta("还有 ", cx);
            panel.stream_delta("**28G**", cx);
        });
        // 一边流一边画：行高在变，虚拟列表得跟着重新量。
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.messages.len(), 1, "一轮的增量该落在同一条消息上");
            assert_eq!(text_of(&panel.messages[0]), "根分区还有 **28G**");
            assert!(panel.streaming.is_some(), "这一轮还没说完");
        });

        panel.update(cx, |panel, cx| {
            panel.end_turn(Some(reply("根分区还有 **28G** 可用。")), cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.messages.len(), 1, "收尾不该再多出一条");
            assert_eq!(
                text_of(&panel.messages[0]),
                "根分区还有 **28G** 可用。",
                "收尾那份是替换，不是接在后面"
            );
            assert!(panel.streaming.is_none(), "说完了就不在流里了");
        });

        panel.update(cx, |panel, cx| panel.stream_delta("再看看内存", cx));
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.messages.len(), 2, "下一轮该另起一条");
            assert_eq!(text_of(&panel.messages[1]), "再看看内存");
        });
    }

    /// 复制键：画得出来，真按下去，消息原文就在剪贴板里。
    ///
    /// 断言的是 **Markdown 源码**而不是画出来的那个样子（`**28G**` 要带着
    /// 星号进剪贴板）：这段话多半要粘进工单或者另一个聊天窗口，那些地方认的
    /// 正是源码。
    ///
    /// 这条走 [`root_harness`] 而不是 [`dock_harness`]：要按下去的东西，
    /// 边界表里得查得到（原因见那个函数）。
    #[gpui_kit::test]
    fn copying_a_message_puts_its_source_on_the_clipboard(cx: &mut gpui_kit::TestAppContext) {
        const TEXT: &str = "根分区还有 **28G** 可用。";

        let (_root, _panel, cx) = root_harness(
            cx,
            vec![ChatMessage {
                id: "m1".into(),
                role: ChatRole::Assistant,
                body: MessageBody::text(TEXT),
            }],
        );

        let bounds = cx.debug_bounds(COPY_SELECTOR).expect("消息下面没有复制键");
        cx.simulate_click(bounds.center(), gpui_kit::Modifiers::none());

        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some(TEXT), "复制的该是 Markdown 原文");
    }

    /// 回车发送，Shift+回车只换行。
    ///
    /// 输入框是多行的，换行那一下和发送那一下报的是**同一条**
    /// [`InputEvent::PressEnter`]，只差一个 `shift`。漏挑它的话，用户每换一行
    /// 就把写了一半的问题发出去一次 —— 而 `cargo check` 一个字都不会说。
    ///
    /// 配置摆成"填好了"，不然发送在开头就被拦下，两种按法看起来一模一样。
    /// 真跑起来要连模型，所以拿到证据（消息落了、面板转成运行中）就把那场
    /// 运行掐掉 —— 它还没被轮询过，丢掉就等于没发生。
    #[gpui_kit::test]
    fn shift_enter_breaks_the_line_instead_of_sending(cx: &mut gpui_kit::TestAppContext) {
        let _exclusive = assistant_config::exclusive_for_test();
        assistant_config::set_global(assistant_config::single_model_for_test("test-model", false));

        let (_root, panel, cx) = root_harness(cx, Vec::new());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.composer.update(cx, |composer, cx| {
                    composer.set_value("看看磁盘还剩多少", window, cx);
                });
            });
        });

        let press_enter = |cx: &mut gpui_kit::VisualTestContext, shift: bool| {
            panel.update(cx, |panel, cx| {
                panel.composer.update(cx, |_, cx| {
                    cx.emit(InputEvent::PressEnter {
                        secondary: false,
                        shift,
                    });
                });
            });
        };

        press_enter(cx, true);
        panel.read_with(cx, |panel, _| {
            assert!(panel.messages.is_empty(), "Shift+回车只换行，不该发出去");
            assert!(!panel.running);
        });

        press_enter(cx, false);
        panel.update(cx, |panel, _| {
            assert_eq!(panel.messages.len(), 1, "回车该把这句发出去");
            assert!(panel.running);
            panel._run = None;
        });
    }

    /// 在输入框里粘贴一张图（Ctrl+V 与右键菜单走的是同一个 `Paste` 动作）：
    /// 它该变成输入框上方的一张缩略图，规整到模型收的尺寸，而**不是**一串字
    /// 插进输入框；按缩略图上的移除键，它就没了。
    ///
    /// 走的是真的那条路：往剪贴板里放图、焦点放进输入框、派发 `Paste`，由组件库
    /// 的 `on_paste` 钩子交到面板手上 —— 钩子没接上的话，这里什么都收不到。
    #[gpui_kit::test]
    fn pasting_an_image_attaches_it_instead_of_typing(cx: &mut gpui_kit::TestAppContext) {
        let _exclusive = assistant_config::exclusive_for_test();
        assistant_config::set_global(assistant_config::single_model_for_test("test-model", true));

        let (_root, panel, cx) = root_harness(cx, Vec::new());
        paste_image(cx, &panel, png_bytes(3136, 100));

        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.drafts.len(), 1, "贴上的图该排到输入框上方");
            let image = panel.drafts[0].image.as_ref().expect("后台规整完该有图");
            assert_eq!(
                (image.width(), image.height()),
                (assistant::image::MAX_EDGE, 50),
                "该缩到模型收的尺寸"
            );
            assert!(
                panel.composer.read(cx).value().is_empty(),
                "图不该变成一串字插进输入框"
            );
        });

        cx.update(|window, cx| window.draw(cx).clear(cx));
        let remove = cx
            .debug_bounds(DRAFT_REMOVE_SELECTOR)
            .expect("缩略图上没有移除键");
        cx.simulate_click(remove.center(), gpui_kit::Modifiers::none());
        panel.read_with(cx, |panel, _| {
            assert!(panel.drafts.is_empty(), "按了移除键图还在");
        });
    }

    /// 模型没开看图：贴图一张都不收（只说一声去设置里打开），输入框里也不会
    /// 多出一串字。
    #[gpui_kit::test]
    fn a_text_only_model_takes_no_images(cx: &mut gpui_kit::TestAppContext) {
        let _exclusive = assistant_config::exclusive_for_test();
        assistant_config::set_global(assistant_config::AssistantConfig::default());

        let (_root, panel, cx) = root_harness(cx, Vec::new());
        paste_image(cx, &panel, png_bytes(32, 32));

        panel.read_with(cx, |panel, cx| {
            assert!(panel.drafts.is_empty(), "没开看图不该收图");
            assert!(panel.composer.read(cx).value().is_empty());
        });
    }

    /// 只贴了图、一个字没写也能发：那条提问带着图上屏，输入行上的图清空。
    /// 还有图没规整完时不许发 —— 发出去的得是用户在缩略图上看到的那一张。
    #[gpui_kit::test]
    fn sending_takes_the_images_along(cx: &mut gpui_kit::TestAppContext) {
        let _exclusive = assistant_config::exclusive_for_test();
        assistant_config::set_global(assistant_config::single_model_for_test("test-model", true));

        let (_root, panel, cx) = root_harness(cx, Vec::new());
        let press_enter = |cx: &mut gpui_kit::VisualTestContext| {
            panel.update(cx, |panel, cx| {
                panel.composer.update(cx, |_, cx| {
                    cx.emit(InputEvent::PressEnter {
                        secondary: false,
                        shift: false,
                    });
                });
            });
        };

        panel.update(cx, |panel, _| {
            panel.drafts.push(Draft {
                id: "d1".into(),
                image: None,
            });
        });
        press_enter(cx);
        panel.read_with(cx, |panel, _| {
            assert!(panel.messages.is_empty(), "还在规整的图不该跟着发出去");
            assert_eq!(panel.drafts.len(), 1);
        });

        panel.update(cx, |panel, _| panel.drafts[0].image = Some(sample_image()));
        press_enter(cx);
        panel.update(cx, |panel, _| {
            assert!(panel.drafts.is_empty(), "发出去之后输入行上的图该清空");
            let Some(ChatMessage {
                role: ChatRole::User,
                body: MessageBody::Text { text, images, .. },
                ..
            }) = panel.messages.last()
            else {
                panic!("该有一条提问上屏");
            };
            assert!(text.is_empty());
            assert_eq!(images, &vec![sample_image()]);
            assert!(panel.running);
            panel._run = None;
        });
    }

    /// 一轮的规范全文：只有正文。
    fn reply(text: &str) -> TurnOutput {
        TurnOutput {
            text: text.into(),
            ..Default::default()
        }
    }

    /// 思考：一边想一边摊开着写灰字（增量落在同一条上）；一开口说话它就收
    /// 起来，回复另起一条；这一轮收尾时两条按先后交出去落库。
    ///
    /// 查的是画出来的那一帧：用户要看到的是"想的时候看得见、想完自己收起"，
    /// 集合对了而 `when` 写反、标题传不到开合，正文照样会摆在那儿。
    #[gpui_kit::test]
    fn thinking_streams_open_and_folds_once_the_answer_starts(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (_root, panel, cx) = root_harness(cx, Vec::new());

        panel.update(cx, |panel, cx| {
            panel.reasoning_delta("用户问磁盘，", cx);
            panel.reasoning_delta("先跑 df。", cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.messages.len(), 1, "一段思考的增量该落在同一条上");
            assert_eq!(
                panel.messages[0].body,
                MessageBody::Reasoning {
                    text: "用户问磁盘，先跑 df。".into()
                }
            );
        });
        assert!(
            cx.debug_bounds(REASONING_BODY_SELECTOR).is_some(),
            "想的时候思考该摊开着"
        );

        panel.update(cx, |panel, cx| panel.stream_delta("根分区还有 28G", cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.messages.len(), 2, "回复该另起一条");
            assert!(panel.expanded.is_empty(), "一开口说话思考就该收起来");
        });
        assert!(
            cx.debug_bounds(REASONING_BODY_SELECTOR).is_none(),
            "开口说话之后思考还摊着"
        );

        let finished = panel.update(cx, |panel, cx| {
            panel.end_turn(
                Some(TurnOutput {
                    reasoning: "用户问磁盘，先跑 df。".into(),
                    text: "根分区还有 28G 可用。".into(),
                    tokens_per_sec: Some(30.0),
                }),
                cx,
            )
        });
        assert_eq!(
            finished,
            [
                MessageBody::Reasoning {
                    text: "用户问磁盘，先跑 df。".into()
                },
                MessageBody::reply("根分区还有 28G 可用。", Some(30.0)),
            ],
            "收尾要按屏上的先后交出两条去落库"
        );
        panel.read_with(cx, |panel, _| {
            assert!(panel.reasoning.is_none() && panel.streaming.is_none());
            assert_eq!(panel.messages.len(), 2, "收尾是替换，不多出一条");
        });
    }

    /// 带工具的一场在屏上必须是"思考 → 工具卡片 → 思考 → 回答"。
    ///
    /// 线上栽过（读终端那一场）：第一轮模型叫工具之前先吐了一个 `"\n\n"`，
    /// 面板为它开了一条正文，而那一轮的收尾因为正文是空白的根本没来 ——
    /// 第二轮的回答于是接到了那条空白的尾巴上，排到了工具卡片**前面**，第二轮
    /// 的思考反倒落在了最后，回答上方还空着一块。这里照着那一场的事件先后
    /// 原样回放一遍。
    #[gpui_kit::test]
    fn a_tool_run_reads_think_card_think_answer(cx: &mut gpui_kit::TestAppContext) {
        let (_root, panel, cx) = root_harness(cx, Vec::new());

        let finished = panel.update(cx, |panel, cx| {
            let mut finished = Vec::new();
            // 第一轮：想 → 一个空白的正文 → 叫工具 → 收尾（正文报成空串）。
            panel.reasoning_delta("用户要看终端在跑什么，", cx);
            panel.stream_delta("\n\n", cx);
            finished.extend(panel.end_turn(
                Some(TurnOutput {
                    reasoning: "用户要看终端在跑什么，".into(),
                    ..Default::default()
                }),
                cx,
            ));
            // 工具跑完，卡片上屏。
            panel.push_message(
                ChatMessage {
                    id: "card".into(),
                    role: ChatRole::Assistant,
                    body: MessageBody::Command {
                        command: "读取终端输出（最近 30 行）".into(),
                        session: "web-01".into(),
                        output: "PING qq.com".into(),
                        status: CommandStatus::Ok,
                    },
                },
                cx,
            );
            // 第二轮：再想 → 回答 → 收尾。
            panel.reasoning_delta("是 ping qq.com。", cx);
            panel.stream_delta("你当前执行的命令是 ping qq.com。", cx);
            finished.extend(panel.end_turn(
                Some(TurnOutput {
                    reasoning: "是 ping qq.com。".into(),
                    text: "你当前执行的命令是 ping qq.com。".into(),
                    tokens_per_sec: Some(25.7),
                }),
                cx,
            ));
            finished
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let shape = |body: &MessageBody| match body {
            MessageBody::Reasoning { .. } => "思考",
            MessageBody::Command { .. } | MessageBody::Transfer(_) => "卡片",
            MessageBody::Text { .. } => "回答",
        };
        panel.read_with(cx, |panel, _| {
            let order: Vec<_> = panel.messages.iter().map(|m| shape(&m.body)).collect();
            assert_eq!(
                order,
                ["思考", "卡片", "思考", "回答"],
                "回答跑到卡片前面、或者多出一块空白了"
            );
        });
        // 落库的先后（卡片由泵另外落，不在收尾交出来的这几条里）。
        assert_eq!(
            finished.iter().map(shape).collect::<Vec<_>>(),
            ["思考", "思考", "回答"]
        );
    }

    /// 模型首字迟迟不来时，停止要直接掐断等待，不能等下一次边界检查。
    #[gpui_kit::test]
    fn stopping_aborts_a_run_waiting_for_the_model(cx: &mut gpui_kit::TestAppContext) {
        let (_root, panel, cx) = root_harness(cx, Vec::new());
        let run = crate::db::runtime().spawn(std::future::pending::<anyhow::Result<()>>());
        panel.update(cx, |panel, cx| {
            panel.running = true;
            panel.run_abort = Some(run.abort_handle());
            panel.stop(cx);
        });

        let error = crate::db::runtime().block_on(run).unwrap_err();
        assert!(error.is_cancelled(), "停止后任务还在等模型返回");
        panel.read_with(cx, |panel, _| {
            assert!(panel.cancel.load(Ordering::Relaxed));
            assert!(panel.run_abort.is_none());
        });
    }

    /// 被停掉、出错时没等到收尾：想到一半、说到一半的照样留下来交去落库，
    /// 两个槽也照样关掉。
    #[gpui_kit::test]
    fn an_interrupted_turn_keeps_what_was_streamed(cx: &mut gpui_kit::TestAppContext) {
        let (_root, panel, cx) = root_harness(cx, Vec::new());
        let finished = panel.update(cx, |panel, cx| {
            panel.reasoning_delta("先看看", cx);
            panel.stream_delta("根分区还", cx);
            panel.end_turn(None, cx)
        });
        assert_eq!(
            finished,
            [
                MessageBody::Reasoning {
                    text: "先看看".into()
                },
                MessageBody::text("根分区还"),
            ]
        );
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.messages.len(), 2);
            assert!(panel.reasoning.is_none() && panel.streaming.is_none());
        });
    }

    /// 模型的回复在复制键旁边写着 tokens/s；没测出速度的（用户的话、老消息）
    /// 那一格不画，不摆一个 0。
    #[gpui_kit::test]
    fn a_reply_shows_its_speed_beside_the_copy_key(cx: &mut gpui_kit::TestAppContext) {
        let (_root, _panel, cx) = root_harness(
            cx,
            vec![ChatMessage {
                id: "m1".into(),
                role: ChatRole::Assistant,
                body: MessageBody::reply("根分区还有 28G。", Some(42.5)),
            }],
        );
        assert!(cx.debug_bounds(COPY_SELECTOR).is_some());
        assert!(
            cx.debug_bounds(SPEED_SELECTOR).is_some(),
            "复制键旁边该写着速度"
        );
        assert_eq!(format!("{:.1} tokens/s", 42.5_f32), "42.5 tokens/s");
    }

    #[gpui_kit::test]
    fn a_reply_without_a_speed_draws_no_speed(cx: &mut gpui_kit::TestAppContext) {
        let (_root, _panel, cx) = root_harness(
            cx,
            vec![ChatMessage {
                id: "m1".into(),
                role: ChatRole::Assistant,
                body: MessageBody::text("根分区还有 28G。"),
            }],
        );
        assert!(cx.debug_bounds(COPY_SELECTOR).is_some());
        assert!(cx.debug_bounds(SPEED_SELECTOR).is_none());
    }

    /// 回复脚注里写着是哪个模型答的；没记模型的（老回复）那一格不画。
    #[gpui_kit::test]
    fn a_reply_names_the_model_that_wrote_it(cx: &mut gpui_kit::TestAppContext) {
        let (_root, _panel, cx) = root_harness(
            cx,
            vec![ChatMessage {
                id: "m1".into(),
                role: ChatRole::Assistant,
                body: MessageBody::reply("根分区还有 28G。", None).with_model("glm-4.5"),
            }],
        );
        assert!(
            cx.debug_bounds(MODEL_SELECTOR).is_some(),
            "复制键旁边该写着模型名"
        );
    }

    #[gpui_kit::test]
    fn an_old_reply_draws_no_model_name(cx: &mut gpui_kit::TestAppContext) {
        let (_root, _panel, cx) = root_harness(
            cx,
            vec![ChatMessage {
                id: "m1".into(),
                role: ChatRole::Assistant,
                body: MessageBody::text("根分区还有 28G。"),
            }],
        );
        assert!(cx.debug_bounds(MODEL_SELECTOR).is_none());
    }

    /// 脚注只落在跑完那一场的最后一条回复上：中间那些“我去查一下”的片段
    /// 不画 —— 复制键、模型名与速度挂在半句话下面，看着就像这场已经说完了；
    /// 正在跑的那一场整段都先不画。
    #[test]
    fn reply_footers_wait_for_the_last_reply_of_a_finished_run() {
        let message = |id: &str, role: ChatRole, body: MessageBody| ChatMessage {
            id: id.into(),
            role,
            body,
        };
        let card = || MessageBody::Command {
            command: "df -h /".into(),
            session: "web-01".into(),
            output: "/dev/vda1  40G  12G  28G  30% /".into(),
            status: CommandStatus::Ok,
        };
        // 第一场：问 → 说一句（叫工具前的话）→ 卡片 → 回答；然后第二场又问。
        let messages = vec![
            message("u1", ChatRole::User, MessageBody::text("看看磁盘")),
            message(
                "a1",
                ChatRole::Assistant,
                MessageBody::reply("我来查一下。", Some(40.0)),
            ),
            message("a2", ChatRole::Assistant, card()),
            message(
                "a3",
                ChatRole::Assistant,
                MessageBody::reply("还剩 28G。", Some(30.0)),
            ),
            message("u2", ChatRole::User, MessageBody::text("那内存呢")),
            message(
                "a4",
                ChatRole::Assistant,
                MessageBody::reply("也够。", Some(50.0)),
            ),
        ];
        assert_eq!(reply_footers(&messages, false), HashSet::from([3, 5]));

        // 第二场还在跑：只有它那一段不画，第一场的脚注照旧。
        assert_eq!(reply_footers(&messages[..5], true), HashSet::from([3]));
        // 停在工具卡片上的那一场：最后一条不是回复，就没有脚注。
        assert!(reply_footers(&messages[..3], false).is_empty());
    }

    /// 一场还没跑完的运行里，屏上的回复下面先不摆脚注 —— 复制键、模型名、
    /// 速度都是“这条回复说完了”的记号，要等这一场收口才落。
    #[gpui_kit::test]
    fn a_running_reply_draws_no_footer_yet(cx: &mut gpui_kit::TestAppContext) {
        let (_root, panel, cx) = root_harness(
            cx,
            vec![
                ChatMessage {
                    id: "m1".into(),
                    role: ChatRole::User,
                    body: MessageBody::text("看看磁盘还剩多少"),
                },
                ChatMessage {
                    id: "m2".into(),
                    role: ChatRole::Assistant,
                    body: MessageBody::reply("我来看看。", Some(42.5)).with_model("glm-4.5"),
                },
            ],
        );
        panel.update(cx, |panel, cx| {
            panel.running = true;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds(SPEED_SELECTOR).is_none(),
            "这一场还没跑完，不该先摆出速度"
        );
        assert!(
            cx.debug_bounds(MODEL_SELECTOR).is_none(),
            "这一场还没跑完，不该先摆出模型名"
        );
    }

    /// 一轮收尾时，回复记上这一场发起时用的模型 —— 跑到一半在输入行上换了
    /// 模型，这一场的回复也仍是原来那个模型说的。
    #[gpui_kit::test]
    fn a_finished_turn_is_tagged_with_the_run_model(cx: &mut gpui_kit::TestAppContext) {
        let (_root, panel, cx) = root_harness(cx, Vec::new());
        let finished = panel.update(cx, |panel, cx| {
            panel.run_model = "model-a".into();
            panel.model_id = "model-b".into();
            panel.stream_delta("根分区还有 28G。", cx);
            panel.end_turn(Some(reply("根分区还有 28G。")), cx)
        });
        assert_eq!(
            finished,
            [MessageBody::text("根分区还有 28G。").with_model("model-a")]
        );
    }

    /// 两家服务商、三个模型的配置：A 认两档思考，B 一档不认，C 只认开/关。
    fn three_models() -> assistant_config::AssistantConfig {
        use crate::assistant::{ModelConfig, Provider};
        let model = |id: &str, efforts: Vec<ThinkingEffort>| ModelConfig {
            id: id.into(),
            name: format!("name-{id}"),
            vision: false,
            efforts,
            max_tokens: None,
        };
        assistant_config::AssistantConfig {
            providers: vec![
                Provider {
                    id: "p1".into(),
                    api_key: "sk-1".into(),
                    models: vec![
                        model("a", vec![ThinkingEffort::Low, ThinkingEffort::High]),
                        model("b", Vec::new()),
                    ],
                    ..Default::default()
                },
                Provider {
                    id: "p2".into(),
                    api_key: "sk-2".into(),
                    models: vec![model("c", vec![ThinkingEffort::None, ThinkingEffort::High])],
                    ..Default::default()
                },
            ],
            default_model: "a".into(),
            default_effort: Some(ThinkingEffort::Low),
            auto_execute: false,
        }
    }

    /// 模型与思考强度按对话各记各的：在一段对话里换了的，换去别的对话再回来
    /// 还是它；新开的空白对话从"最近挑的那个"起步。
    #[gpui_kit::test]
    fn each_conversation_keeps_its_own_model(cx: &mut gpui_kit::TestAppContext) {
        let _exclusive = assistant_config::exclusive_for_test();
        assistant_config::set_global(three_models());
        let (_root, panel, cx) = root_harness(cx, Vec::new());

        let chat = |id: &str, model_id: &str, effort| Conversation {
            id: SharedString::from(id.to_owned()),
            title: "t".into(),
            host_id: "host".into(),
            model_id: SharedString::from(model_id.to_owned()),
            effort,
        };
        // 照 `open_conversation` 换对话，只是不去库里读消息：那一步在 tokio
        // 线程上唤醒界面任务，测试调度器会判它不确定（并行跑满时必炸）。
        let switch_to = |panel: &mut AssistantPanel, id: &str| {
            let id = SharedString::from(id.to_owned());
            panel.adopt_conversation(&id);
            panel.active = Some(id);
        };
        panel.update(cx, |panel, cx| {
            panel.conversations = vec![
                chat("chat-1", "a", Some(ThinkingEffort::High)),
                chat("chat-2", "c", Some(ThinkingEffort::None)),
                // 升级前的旧对话：没记过模型，用默认。
                chat("chat-3", "", None),
            ];
            switch_to(panel, "chat-1");
            assert_eq!(
                (panel.model_id.as_ref(), panel.effort),
                ("a", Some(ThinkingEffort::High))
            );
            switch_to(panel, "chat-2");
            assert_eq!(
                (panel.model_id.as_ref(), panel.effort),
                ("c", Some(ThinkingEffort::None))
            );
            switch_to(panel, "chat-3");
            assert_eq!(
                (panel.model_id.as_ref(), panel.effort),
                ("a", Some(ThinkingEffort::Low)),
                "没记过的旧对话用默认"
            );

            // 在 chat-2 里换成 B：记到 chat-2 上，也成了新对话的起点。
            switch_to(panel, "chat-2");
            panel.choose_model("b".into(), cx);
            let chat2 = panel
                .conversations
                .iter()
                .find(|chat| chat.id.as_ref() == "chat-2")
                .unwrap();
            assert_eq!(chat2.model_id.as_ref(), "b");
            assert_eq!(assistant_config::global().default_model, "b");

            switch_to(panel, "chat-1");
            assert_eq!(panel.model_id.as_ref(), "a", "chat-1 不受 chat-2 的影响");

            panel.new_conversation(cx);
            assert_eq!(panel.model_id.as_ref(), "b", "新对话从最近挑的那个起步");
        });
    }

    /// 换模型时思考强度按新模型认的档位退：A 上的 low 到了只认开/关的 C
    /// 上退成最强的 high；换到一档不认的 B 不丢原来那一档，换回来还在。
    #[gpui_kit::test]
    fn switching_models_settles_the_effort(cx: &mut gpui_kit::TestAppContext) {
        let _exclusive = assistant_config::exclusive_for_test();
        assistant_config::set_global(three_models());
        let (_root, panel, cx) = root_harness(cx, Vec::new());
        panel.update(cx, |panel, cx| {
            panel.choose("a".into(), Some(ThinkingEffort::Low), cx);
            panel.choose_model("c".into(), cx);
            assert_eq!(panel.effort, Some(ThinkingEffort::High));
            panel.choose("c".into(), Some(ThinkingEffort::None), cx);
            panel.choose_model("b".into(), cx);
            assert_eq!(panel.effort, Some(ThinkingEffort::None), "B 不认任何档，原样留着");
            panel.choose_model("c".into(), cx);
            assert_eq!(panel.effort, Some(ThinkingEffort::None));
        });
        // 输入行上两颗键都画得出来（C 认思考强度）。
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 读终端只读当前会话：中心区停着的是一个不连任何东西的空壳（本机 Shell
    /// 也是这样 —— 没有 SSH 连接可借），槽里就是空的，模型要读也只拿到"没有"。
    /// 命令落不到的地方，屏上的字也不该被拿走。
    #[gpui_kit::test]
    fn reading_the_terminal_needs_a_live_session(cx: &mut gpui_kit::TestAppContext) {
        let (harness, cx) = dock_harness(cx, Vec::new());
        let (dock_area, panel) = harness.read_with(cx, |harness, _| {
            (harness.dock_area.clone(), harness.assistant.clone())
        });
        cx.update(|window, cx| {
            let session = TerminalPanel::new_detached(dock_area.downgrade(), window, cx);
            dock_area.update(cx, |area, cx| {
                area.add_panel_view(panel_handle(session), DockPlacement::Center, None, window, cx);
            });
        });
        cx.run_until_parked();

        panel.read_with(cx, |panel, cx| {
            assert!(panel.current.is_some(), "中心区那个面板该认出来");
            assert!(panel.session.get().is_none());
            assert_eq!(panel.read_terminal(100, cx), None);
        });
    }

    /// 往剪贴板里放一张图，焦点放进输入框，派发一次 `Paste`，再等后台规整完。
    fn paste_image(
        cx: &mut gpui_kit::VisualTestContext,
        panel: &Entity<AssistantPanel>,
        png: Vec<u8>,
    ) {
        cx.write_to_clipboard(ClipboardItem::new_image(&gpui_kit::Image::from_bytes(
            gpui_kit::ImageFormat::Png,
            png,
        )));
        cx.update(|window, cx| {
            let handle = panel.read(cx).composer.focus_handle(cx);
            window.focus(&handle, cx);
            window.dispatch_action(Box::new(gpui_kit::component::input::Paste), cx);
        });
        cx.run_until_parked();
    }

    /// 命令卡片一上屏是收着的：输出压根不画，点一下卡片头才摊开，再点一下
    /// 收回去。
    ///
    /// 查的是**画出来的那一帧**而不是 `expanded` 那个集合：这条真正要守的
    /// 是"用户一眼看到的是收着的卡片"，而集合空着和输出没画之间隔着整条
    /// 渲染链 —— 卡片头传不到开合状态、`when` 写反，集合都还是对的。
    #[gpui_kit::test]
    fn a_command_card_starts_collapsed(cx: &mut gpui_kit::TestAppContext) {
        let (_root, panel, cx) = root_harness(
            cx,
            vec![ChatMessage {
                id: "m1".into(),
                role: ChatRole::Assistant,
                body: MessageBody::Command {
                    command: "df -h /".into(),
                    session: "web-01".into(),
                    output: "/dev/vda1  40G  12G  28G  30% /".into(),
                    status: CommandStatus::Ok,
                },
            }],
        );

        let head = cx.debug_bounds(CARD_HEAD_SELECTOR).expect("卡片头没画出来");
        assert!(
            cx.debug_bounds(CARD_OUTPUT_SELECTOR).is_none(),
            "卡片一上屏就把输出摊开了"
        );

        cx.simulate_click(head.center(), gpui_kit::Modifiers::none());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.read_with(cx, |panel, _| {
            assert!(panel.expanded.contains(&SharedString::from("m1")));
        });
        assert!(
            cx.debug_bounds(CARD_OUTPUT_SELECTOR).is_some(),
            "点开了还不见输出"
        );

        cx.simulate_click(head.center(), gpui_kit::Modifiers::none());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds(CARD_OUTPUT_SELECTOR).is_none(),
            "再点一下该收回去"
        );
    }

    /// 换标签页就是换机器：助手认的"当前会话"要跟着**中心区**的活动标签页走。
    ///
    /// 这条守的是那条链：Dock 的 `LayoutChanged` → 重认一遍
    /// [`TerminalPanel::active_in`] → 换掉那一格。链上断一环，助手就会一直
    /// 在用户已经离开的那台机器上跑命令，而界面上一个字都不会说。
    ///
    /// 它同时守着另一件事：助手自己也停在这个 Dock 里（右侧 dock），
    /// 但中心区空着的时候它认出来的是"没有会话"，而不是自己。
    ///
    /// 停的是不连任何东西的空壳（没有 SSH 连接可借），所以那一格始终是
    /// `None` —— 这里验的是"认的是哪个面板"，译成目标那一步归
    /// [`TerminalPanel::session_target`] 自己那条测试。
    #[gpui_kit::test]
    fn the_panel_follows_the_active_tab(cx: &mut gpui_kit::TestAppContext) {
        let (harness, cx) = dock_harness(cx, Vec::new());
        let (dock_area, panel) = harness.read_with(cx, |harness, _| {
            (harness.dock_area.downgrade(), harness.assistant.clone())
        });

        // 中心区空着：没有会话在前面，也不该把右侧 dock 里的自己算进去。
        panel.read_with(cx, |panel, _| {
            assert!(panel.current.is_none());
            assert!(panel.session.get().is_none());
        });

        let open = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| {
                let area = dock_area.upgrade().expect("dock 还在");
                let session = TerminalPanel::new_detached(area.downgrade(), window, cx);
                area.update(cx, |area, cx| {
                    area.add_panel_view(
                        panel_handle(session.clone()),
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    );
                });
                session
            })
        };

        let first = open(cx);
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            let current = panel.current.as_ref().expect("停了一条会话就该认出来");
            assert_eq!(current.entity_id(), first.entity_id());
        });

        // 新开的标签页会被选到最前 —— 助手该跟过去。
        let second = open(cx);
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            let current = panel.current.as_ref().expect("还停着两条会话");
            assert_eq!(
                current.entity_id(),
                second.entity_id(),
                "换了活动标签页，助手还停在上一条上"
            );
        });
    }

    /// 把面板当窗口的根视图画两帧，不进 Dock —— 要按下去的测试用这个。
    ///
    /// 不是图省事：Dock 把面板挂成一块 `cached` 子视图，缓存命中的那些帧
    /// 根本不跑 `Interactivity::paint`，而调试边界表每帧都清空 —— 按钮在
    /// 屏幕上好好画着，`debug_bounds` 却查不到它。
    ///
    /// 外面那层 [`Root`] 是给通知层的：复制完要弹一句"已复制"，而
    /// `window.push_notification` 找不到 `Root` 会直接 panic。两帧是给虚拟
    /// 列表的：行的位置要第二帧才量得出来，行里那些东西的边界跟着它。
    ///
    /// [`Root`] 一起还回去：窗口的根视图没人留着，这一帧画的东西就没了。
    fn root_harness(
        cx: &mut gpui_kit::TestAppContext,
        messages: Vec<ChatMessage>,
    ) -> (
        Entity<gpui_kit::component::Root>,
        Entity<AssistantPanel>,
        &mut gpui_kit::VisualTestContext,
    ) {
        use gpui_kit::component::Root;

        let service = AssistantService::in_memory_blocking();
        cx.update(gpui_kit::init);
        let mut panel = None;
        let (root, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("assistant-root-test", None, window, cx));
            let assistant = cx.new(|cx| {
                AssistantPanel::from_snapshot(service, dock_area.downgrade(), messages, window, cx)
            });
            panel = Some(assistant.clone());
            Root::new(assistant, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (root, panel.expect("面板刚建出来"), cx)
    }

    /// 把面板停进右侧 dock 里画一帧，和真实应用一样。
    ///
    /// 光把它当根视图画是不够的：标签条那条路（[`Panel::title`] 的对话菜单、
    /// [`Panel::title_suffix`] 的警示牌、[`Panel::toolbar_buttons`] 的两颗
    /// 按钮）只有真的挂进 `DockArea` 才会走到。标签条样式跟着应用一起钉在
    /// `TabBar` 档。
    fn dock_harness(
        cx: &mut gpui_kit::TestAppContext,
        messages: Vec<ChatMessage>,
    ) -> (Entity<Harness>, &mut gpui_kit::VisualTestContext) {
        let service = AssistantService::in_memory_blocking();
        cx.update(gpui_kit::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock_area, skin) = DockSkin::dock_area("assistant-test-dock", Some(1), window, cx);
            skin.set_panel_style(PanelStyle::TabBar, cx);
            let assistant = cx.new(|cx| {
                AssistantPanel::from_snapshot(service, dock_area.downgrade(), messages, window, cx)
            });
            dock_area.update(cx, |area, cx| {
                area.set_dock(
                    DockPlacement::Right,
                    DockLayout::tabs().panel_view(panel_handle(assistant.clone()), cx),
                    window,
                    cx,
                );
            });
            Harness {
                dock_area,
                assistant,
                _skin: skin,
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (harness, cx)
    }

    /// 测试外壳：留住 Dock（面板只握弱引用，没人留住它就当场没了 ——
    /// 真实应用里留住它的是工作区外壳），把整个 Dock 画出来。
    struct Harness {
        dock_area: Entity<DockArea>,
        assistant: Entity<AssistantPanel>,
        _skin: Rc<gpui_kit::component::dock::DockSkin>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.dock_area.clone())
        }
    }
