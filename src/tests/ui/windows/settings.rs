    use super::*;

    #[gpui_kit::test]
    fn fps_monitor_global_state_sync(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (_shell, cx) = cx.add_window_view(|window, cx| SettingsWindow::new(window, cx));
        cx.update(|_window, cx| assert!(!show_fps_monitor(cx), "默认关闭"));
        cx.update(|window, cx| {
            set_show_fps_monitor(true, Some(window), cx);
            assert!(show_fps_monitor(cx));
        });
        cx.update(|window, cx| {
            set_show_fps_monitor(false, Some(window), cx);
            assert!(!show_fps_monitor(cx));
        });
    }

    /// 配好代理之后那一页也要画得通：开着的时候多出四格输入框，那几格只有
    /// 这一档才走得到，深 builder 链上的错只有真的画一遍才掉得出来。
    ///
    /// 只动进程里那份缓存，不碰服务层 —— 界面测试不该去开用户那座真库。
    #[gpui_kit::test]
    fn the_proxy_page_draws_with_a_proxy_configured(cx: &mut gpui_kit::TestAppContext) {
        let _guard = proxy::exclusive_for_test();
        proxy::set_global(GlobalProxy {
            enabled: true,
            config: ProxyConfig {
                host: "127.0.0.1".into(),
                port: 1080,
                username: "deploy".into(),
                password: "hunter2".into(),
            },
        });

        cx.update(gpui_kit::init);
        let (shell, cx) = cx.add_window_view(SettingsWindow::new);
        shell.update(cx, |shell, cx| shell.set_tab(SettingsTab::Proxy, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        // 第二帧走的不是同一条路：元素状态、焦点都只有这一帧才读得到。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // 四格是照着缓存那一份预填的。
        shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.proxy.host.read(cx).value(), "127.0.0.1");
            assert_eq!(shell.proxy.port.read(cx).value(), "1080");
            assert_eq!(shell.proxy.username.read(cx).value(), "deploy");
            assert_eq!(shell.proxy.password.read(cx).value(), "hunter2");
        });
    }

    /// 挑过字体之后那一页也要画得通。
    ///
    /// 挑中的那个族名多半不在筛出来的清单里（测试用的文字系统一个字体都
    /// 报不出来），[`SettingsWindow::terminal_page`] 得把它补进下拉 ——
    /// 那条补齐的路只有真的画一遍才走得到。
    ///
    /// 只动进程里那份缓存，不碰服务层。
    #[gpui_kit::test]
    fn the_terminal_page_draws_with_a_font_picked(cx: &mut gpui_kit::TestAppContext) {
        let _guard = font::exclusive_for_test();
        font::set_global(TerminalFont {
            family: "JetBrains Mono".into(),
            size: 18.,
        });

        cx.update(gpui_kit::init);
        let (shell, cx) = cx.add_window_view(SettingsWindow::new);
        shell.update(cx, |shell, cx| shell.set_tab(SettingsTab::Terminal, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        // 第二帧走的不是同一条路：数字输入框的状态只有这一帧才读得到。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert_eq!(font::global().family, "JetBrains Mono", "画一遍不该改掉它");
        assert_eq!(font::global().size, 18.);
    }

    /// 配色下拉画得通，而且那一列里每一套内置方案都在。少一套的话它就永远
    /// 挑不上 —— 那种缺失从"页面画出来了"上是看不出来的。
    #[gpui_kit::test]
    fn the_scheme_dropdown_lists_every_preset(cx: &mut gpui_kit::TestAppContext) {
        let _guard = scheme::exclusive_for_test();
        scheme::set_global("gruvbox_dark");

        cx.update(gpui_kit::init);
        let (shell, cx) = cx.add_window_view(SettingsWindow::new);
        shell.update(cx, |shell, cx| shell.set_tab(SettingsTab::Terminal, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert_eq!(scheme::global(), "gruvbox_dark", "画一遍不该改掉它");

        // 下拉那一列是在这儿拼的，组件那边只拿得到拼好的结果，所以照着
        // 同一段逻辑核对一遍。
        let options = SettingsWindow::scheme_options();
        assert_eq!(options[0].0, "", "第一项是跟随界面主题");
        for preset in &scheme::PRESETS {
            assert!(
                options
                    .iter()
                    .any(|(id, name)| id == preset.id && name == preset.name),
                "{} 不在下拉里",
                preset.id
            );
        }
    }

    /// 挑中一家预设厂商之后，那家的地址要真的落进 endpoint 那一格。
    ///
    /// 这一下在真机上是从下拉的弹出菜单里发出来的：那一刻正跑在设置窗口自己
    /// 的事件派发里，GPUI 已经把 `Window` 从窗口表里取走了，当场再借一次只
    /// 拿得到 `Err` —— 而那个 `Err` 是被 `_ =` 吞掉的，于是输入框一个字都没
    /// 填上，界面上什么也看不出来。测试照同一条路走：在 `window.update` 里
    /// 调，回填得能穿过去（见 [`defer_with_settings_window`]）。
    ///
    /// 只走回填这一半，不走 [`BackupInputs::apply_s3_preset`] 的落库那一半：
    /// 后者派到后台的任务会去开用户真正那座库。
    #[gpui_kit::test]
    fn picking_an_s3_preset_fills_the_endpoint_box(cx: &mut gpui_kit::TestAppContext) {
        let _guard = backup::exclusive_for_test();
        backup::set_global(BackupConfig {
            protocol: BackupProtocol::S3,
            s3: S3Config {
                endpoint: String::new(),
                region: String::new(),
                ..S3Config::default()
            },
            ..BackupConfig::default()
        });

        cx.update(gpui_kit::init);

        // 照着 [`SettingsWindow::open`] 搭：单例句柄拿的是 `Root` 那一层，
        // 借窗口那条路只有这么搭才走得到。
        let shell_slot: std::rc::Rc<std::cell::RefCell<Option<Entity<SettingsWindow>>>> =
            Default::default();
        let window = cx.add_window({
            let shell_slot = shell_slot.clone();
            move |window, cx| {
                let shell = cx.new(|cx| SettingsWindow::new(window, cx));
                *shell_slot.borrow_mut() = Some(shell.clone());
                Root::new(shell, window, cx)
            }
        });
        cx.update(|cx| cx.set_global(ActiveSettingsWindow(window)));
        let shell = shell_slot.borrow().clone().expect("外壳建起来了");

        let aliyun = backup::find_s3_preset("aliyun").expect("内置预设里有阿里云");
        window
            .update(cx, |_, _window, cx| {
                let inputs = shell.read(cx).backup.clone();
                assert_eq!(inputs.s3_endpoint.read(cx).value(), "", "起手是空的");
                inputs.fill_s3_endpoint_and_region(
                    aliyun.default_endpoint,
                    aliyun.default_region,
                    cx,
                );
            })
            .expect("设置窗口还开着");

        window
            .update(cx, |_, _window, cx| {
                let inputs = shell.read(cx).backup.clone();
                assert_eq!(
                    inputs.s3_endpoint.read(cx).value(),
                    aliyun.default_endpoint,
                    "厂商地址没回填"
                );
                assert_eq!(inputs.s3_region.read(cx).value(), aliyun.default_region);
            })
            .expect("设置窗口还开着");
    }

    /// 设置窗口得把弹层画出来。
    ///
    /// `window.open_dialog` 只把弹窗记到 `Root` 上，真让它上屏的是窗口第一层
    /// 那个 `Root` 自己（0.6.5 起弹层统一由 `Root` 挂，各扇窗的 render 里不再
    /// 需要那三句 `Root::render_*_layer`）。漏掉 `Root` 这一层，弹窗照开，
    /// 只是谁也看不见 —— 「从云端恢复」那颗按钮当初就这么看着像坏了。
    /// 所以这里断的不是「开没开」，是「画没画出来」。
    #[gpui_kit::test]
    fn the_settings_window_draws_the_dialog_layer(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);

        // 照着 [`SettingsWindow::open`] 搭：弹层归 `Root` 存，得有那一层才验得到。
        let window = cx.add_window(|window, cx| {
            let shell = cx.new(|cx| SettingsWindow::new(window, cx));
            Root::new(shell, window, cx)
        });

        // 开弹窗走 `VisualTestContext`，不走 `window.update`：后者进的是 `Root`
        // 自己那层实体更新，`open_dialog` 在里面再 `Root::update` 一次就炸。
        let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| {
            assert!(!window.has_active_dialog(cx), "起手不该有弹窗");
            window.open_dialog(cx, |dialog, _, _| dialog.title("云端副本管理"));
            assert!(window.has_active_dialog(cx), "弹窗没记到 Root 上");
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            visual.debug_bounds("dialog-layer").is_some(),
            "弹层没画出来：弹窗开着也上不了屏，点按钮和按钮坏了长得一样"
        );
    }

    #[gpui_kit::test]
    fn settings_window_draws_all_tabs(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (shell, cx) = cx.add_window_view(|window, cx| SettingsWindow::new(window, cx));

        // 四个 tab 都要画得出来，不崩。
        for tab in SettingsTab::ALL {
            shell.update(cx, |shell, cx| {
                shell.set_tab(tab, cx);
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    /// 配了两家服务商的助手页也要画得通：服务商下拉、Completions 协议才有的
    /// "思考开关参数"、模型列表那几行，只有真配了东西才走得到。三格输入框照
    /// 选中的那一家预填，换一家就照那一家重填。
    ///
    /// 只动进程里那份缓存，不碰服务层。
    #[gpui_kit::test]
    fn the_assistant_page_draws_providers_and_models(cx: &mut gpui_kit::TestAppContext) {
        use crate::assistant::{ModelConfig, ThinkingEffort};

        let _guard = assistant_config::exclusive_for_test();
        assistant_config::set_global(AssistantConfig {
            providers: vec![
                Provider {
                    id: "relay".into(),
                    name: "Relay".into(),
                    protocol: ProviderProtocol::OpenAi,
                    base_url: "https://relay.example.com/v1".into(),
                    api_key: "sk-relay".into(),
                    thinking_switch: ThinkingSwitch::EnableThinking,
                    models: vec![
                        ModelConfig {
                            id: "a".into(),
                            name: "model-a".into(),
                            vision: true,
                            efforts: vec![ThinkingEffort::Low, ThinkingEffort::High],
                            max_tokens: Some(64_000),
                        },
                        ModelConfig {
                            id: "b".into(),
                            name: "model-b".into(),
                            ..Default::default()
                        },
                    ],
                },
                Provider {
                    id: "claude".into(),
                    protocol: ProviderProtocol::Anthropic,
                    api_key: "sk-ant".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        cx.update(gpui_kit::init);
        let (shell, cx) = cx.add_window_view(SettingsWindow::new);
        shell.update(cx, |shell, cx| shell.set_tab(SettingsTab::Assistant, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.assistant.name.read(cx).value(), "Relay");
            assert_eq!(shell.assistant.api_key.read(cx).value(), "sk-relay");
        });

        // 换到第二家（没有模型、不是 Completions 协议）：三格重填，页面照画。
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.select_assistant_provider(Some("claude".into()), window, cx)
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.assistant.name.read(cx).value(), "");
            assert_eq!(shell.assistant.api_key.read(cx).value(), "sk-ant");
        });
    }

    /// 改名只写选中的那一家：换家时重填不发 Change，不会把上一家的值写过来。
    #[gpui_kit::test]
    fn typing_writes_only_the_selected_provider(cx: &mut gpui_kit::TestAppContext) {
        let _guard = assistant_config::exclusive_for_test();
        let provider = |id: &str| Provider {
            id: id.into(),
            name: id.into(),
            base_url: format!("https://{id}.example.com"),
            ..Default::default()
        };
        assistant_config::set_global(AssistantConfig {
            providers: vec![provider("one"), provider("two")],
            ..Default::default()
        });

        cx.update(gpui_kit::init);
        let (shell, cx) = cx.add_window_view(SettingsWindow::new);
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.select_assistant_provider(Some("two".into()), window, cx);
                shell.assistant.name.update(cx, |input, cx| {
                    input.set_value("Two!", window, cx);
                    cx.emit(InputEvent::Change);
                });
            });
        });
        let config = assistant_config::global();
        assert_eq!(config.providers[0].name, "one");
        assert_eq!(config.providers[0].base_url, "https://one.example.com");
        assert_eq!(config.providers[1].name, "Two!");
        assert_eq!(config.providers[1].base_url, "https://two.example.com");
    }

    /// 模型列表那一行小字里的最大输出写成短的：三个快捷值都正好是整 k。
    #[test]
    fn max_tokens_read_short_in_the_model_list() {
        assert_eq!(short_tokens(8_192), "8k");
        assert_eq!(short_tokens(64_000), "64k");
        assert_eq!(short_tokens(128_000), "128k");
        assert_eq!(short_tokens(12_345), "12345");
    }
