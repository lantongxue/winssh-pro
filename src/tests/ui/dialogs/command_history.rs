    use super::*;
    use crate::host::{AuthMethod, Host, MonitorIntervals};
    use std::cell::RefCell;

    fn host() -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            sftp: Default::default(),
            terminal_scheme: "".into(),
            kind: crate::host::HostKind::Ssh,
            id: "prod-k8s".into(),
            name: "Prod-K8s-Master-01".into(),
            user: "root".into(),
            hostname: "192.168.1.100".into(),
            port: 22,
            platform: "Ubuntu 24.04".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            shell_integration: false,
            favorites: Vec::new(),
        }
    }

    /// 三条历史，把那一列的三种样子都占上：成了、败了、不知道。
    fn records() -> Vec<CommandRecord> {
        [
            ("docker compose up -d", Some(0)),
            ("cat /etc/nginx/missing.conf", Some(1)),
            ("tail -f /var/log/syslog", None),
        ]
        .into_iter()
        .enumerate()
        .map(|(at, (command, exit_code))| CommandRecord {
            id: format!("cmd-{at}").into(),
            command: command.into(),
            exit_code,
            executed_at: "2026-09-12 10:24:31".into(),
        })
        .collect()
    }

    struct Harness {
        dialog: Entity<CommandHistoryDialog>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            v_flex()
                .size_full()
                .child(render_header(&host(), cx))
                .child(self.dialog.clone())
        }
    }

    /// 把对话框挂进窗口画一帧。
    ///
    /// 外面套一层 [`Root`]：[`CommandHistoryDialog::run`] 要收起对话框层，
    /// 窗口第一层不是 `Root` 的话那一句直接 panic。里面再套一层 `Harness`，
    /// 是为了把标题那一段也画上 —— 它不在 [`Render`] 里（交给对话框外壳的），
    /// 单画正文覆盖不到，而它同样是一条会读实体状态的 builder 链。
    ///
    /// 快照是**直接塞进去**的，不走 `load` —— 那一步要跨到数据层的 tokio
    /// runtime 上，而 gpui 的测试调度器会把任何来自别的线程的唤醒判成不确定。
    /// 这里要验的是"这份数据画不画得出来"，读库归服务层那几个用例。
    fn draw(
        cx: &mut gpui_kit::TestAppContext,
        records: Vec<CommandRecord>,
        on_run: HistoryRunHandler,
    ) -> (
        Entity<CommandHistoryDialog>,
        &mut gpui_kit::VisualTestContext,
    ) {
        draw_with(cx, records, on_run, Rc::new(|_, _, _| {}))
    }

    /// 同上，但连 shell 集成那个开关的回调也接出来。
    fn draw_with(
        cx: &mut gpui_kit::TestAppContext,
        records: Vec<CommandRecord>,
        on_run: HistoryRunHandler,
        on_shell_integration: ShellIntegrationHandler,
    ) -> (
        Entity<CommandHistoryDialog>,
        &mut gpui_kit::VisualTestContext,
    ) {
        use gpui_kit::component::Root;

        // 开窗之前把这一下阻塞做完：之后一次异步都不能再发。
        let service = CommandHistoryService::in_memory_blocking();

        cx.update(gpui_kit::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let dialog = cx.new(|cx| {
                let mut this = CommandHistoryDialog::new(
                    host(),
                    service,
                    on_run,
                    on_shell_integration,
                    window,
                    cx,
                );
                this.records = records;
                this.loading = false;
                this.apply_filter(cx);
                this
            });
            Root::new(cx.new(|_| Harness { dialog }), window, cx)
        });
        let harness = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<Harness>()
                .expect("刚放进去的就是它")
        });
        let dialog = harness.read_with(cx, |harness, _| harness.dialog.clone());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (dialog, cx)
    }

    #[gpui_kit::test]
    fn the_dialog_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (_, cx) = draw(cx, records(), Rc::new(|_, _, _| {}));
        // 再画一帧：第一帧和后续帧走的不是同一条路（元素状态、焦点都只有
        // 第二帧才读得到），虚拟列表尤其如此 —— 可见区间是量过一帧才有的。
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 一条都没有时画的是另一条分支（空状态那句话），它同样要画得出来。
    #[gpui_kit::test]
    fn an_empty_history_draws_its_empty_state(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw(cx, Vec::new(), Rc::new(|_, _, _| {}));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        dialog.read_with(cx, |this, _| {
            // 读完库、确实一条没有 —— 这句话要点破"对端 shell 得打标记"，
            // 不然用户只会以为功能坏了。
            assert_eq!(
                this.empty_note(),
                SharedString::from(t!("command_history.empty"))
            );
        });
    }

    /// 还在读库时说的是"正在读取"，不是"没有历史"。两件事差得远。
    #[gpui_kit::test]
    fn a_dialog_that_is_still_loading_does_not_claim_to_be_empty(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (dialog, cx) = draw(cx, Vec::new(), Rc::new(|_, _, _| {}));
        cx.update(|_, cx| {
            dialog.update(cx, |this, _| this.loading = true);
        });
        dialog.read_with(cx, |this, _| {
            assert_eq!(
                this.empty_note(),
                SharedString::from(t!("command_history.loading"))
            );
        });
    }

    /// 搜索在快照上过滤，不回库里查。
    #[gpui_kit::test]
    fn the_filter_narrows_the_list(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw(cx, records(), Rc::new(|_, _, _| {}));

        dialog.update_in(cx, |this, window, cx| {
            this.search
                .update(cx, |input, cx| input.set_value("nginx", window, cx));
            // 程序改输入框的值不发 `Change`（那是敲键盘才有的事），
            // 所以这里照着订阅回调里那一句自己叫一次。
            this.apply_filter(cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        dialog.read_with(cx, |this, _| {
            assert_eq!(this.visible.len(), 1, "只有一条命令里带 nginx");
            assert_eq!(
                this.record_at(0).map(|record| record.command.as_ref()),
                Some("cat /etc/nginx/missing.conf")
            );
            // 行高表必须跟着可见行数走，不然虚拟列表算出来的可见区间是错的。
            assert_eq!(this.row_sizes.len(), this.visible.len());
        });
    }

    /// 点一行就是把那条命令发回会话。这是这一层存在的理由。
    #[gpui_kit::test]
    fn clicking_a_row_sends_that_command_back(cx: &mut gpui_kit::TestAppContext) {
        let sent: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let recorder = sent.clone();
        let (dialog, cx) = draw(
            cx,
            records(),
            Rc::new(move |command, _, _| *recorder.borrow_mut() = Some(command)),
        );

        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| this.run(0, window, cx));
        });

        assert_eq!(sent.borrow().as_deref(), Some("docker compose up -d"));
    }

    /// 扳到**开**不算数：先弹一层把那一行摆出来，用户没点头之前什么都不做。
    ///
    /// 这一条钉的是这个功能里最要紧的一句承诺 —— 我们要往用户的服务器上打字，
    /// 那一下必须是他亲手点的。开关自己也不许提前扳过去：它显示的是这台主机
    /// 现在的样子，不是我们打算把它变成的样子。
    #[gpui_kit::test]
    fn turning_it_on_asks_before_typing_anything(cx: &mut gpui_kit::TestAppContext) {
        let reported: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
        let recorder = reported.clone();
        let (dialog, cx) = draw_with(
            cx,
            records(),
            Rc::new(|_, _, _| {}),
            Rc::new(move |on, _, _| *recorder.borrow_mut() = Some(on)),
        );

        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.toggle_shell_integration(true, window, cx)
            });
        });
        // 确认框连同里头那块脚本都要画得出来 —— 用户看不到那一行就没法点头。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(
            cx.update(|window, cx| window.has_active_dialog(cx)),
            "扳到开应当先弹一层确认"
        );
        assert_eq!(*reported.borrow(), None, "还没点头就报上去了");
        dialog.read_with(cx, |this, _| {
            assert!(!this.shell_integration, "开关不该抢在确认之前扳过去");
        });
    }

    /// 点了"装上"才真的算数：开关跟着扳过去，新值也报给宿主。
    #[gpui_kit::test]
    fn confirming_flips_the_switch_and_reports_it(cx: &mut gpui_kit::TestAppContext) {
        let reported: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
        let recorder = reported.clone();
        let (dialog, cx) = draw_with(
            cx,
            records(),
            Rc::new(|_, _, _| {}),
            Rc::new(move |on, _, _| *recorder.borrow_mut() = Some(on)),
        );

        // 确认框那颗 OK 走的就是这一句。
        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.apply_shell_integration(true, window, cx)
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert_eq!(*reported.borrow(), Some(true));
        dialog.read_with(cx, |this, _| assert!(this.shell_integration));
    }

    /// 扳到**关**不弹框：停手不需要许可。
    #[gpui_kit::test]
    fn turning_it_off_takes_effect_at_once(cx: &mut gpui_kit::TestAppContext) {
        let reported: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
        let recorder = reported.clone();
        let (dialog, cx) = draw_with(
            cx,
            records(),
            Rc::new(|_, _, _| {}),
            Rc::new(move |on, _, _| *recorder.borrow_mut() = Some(on)),
        );
        cx.update(|_, cx| {
            dialog.update(cx, |this, _| this.shell_integration = true);
        });

        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.toggle_shell_integration(false, window, cx)
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert_eq!(*reported.borrow(), Some(false));
        assert!(
            !cx.update(|window, cx| window.has_active_dialog(cx)),
            "关掉不该弹任何东西"
        );
        dialog.read_with(cx, |this, _| assert!(!this.shell_integration));
    }

    /// 开关开着时画的是另一条分支（说明文字换了一句），它同样要画得出来。
    #[gpui_kit::test]
    fn the_switch_draws_in_both_positions(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw(cx, records(), Rc::new(|_, _, _| {}));
        cx.update(|_, cx| {
            dialog.update(cx, |this, cx| {
                this.shell_integration = true;
                cx.notify();
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    // 「清空」和行尾那颗「删除」都没有界面层的用例：
    // [`CommandHistoryDialog::clear`] 与 [`CommandHistoryDialog::delete`] 都会派
    // 一次落库到数据层的 tokio runtime 上，而 gpui 的测试调度器一看到来自别的
    // 线程的唤醒就判这个测试不确定（整个测试进程 abort，不只是这一条挂）。
    // 这两件事本身归服务层那两条用例
    // （`CommandHistoryService::tests::clearing_one_host_leaves_the_others_alone`、
    // `…::deleting_one_record_leaves_its_neighbours`）。
