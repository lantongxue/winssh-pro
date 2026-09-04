//! 会话端口转发对话框（原型里的 `SessionPortForwardModal`，快捷键 `Alt+P`）。
//!
//! 一屏里有两件事：**改规则**和**看隧道**。它们的正本都不在这里 ——
//! 规则的正本在主机库那一行（改完经 `on_change` 交回
//! [`crate::ui::panels::TerminalPanel`] 落库），隧道的正本在
//! [`ForwardService`] 里。所以关掉这个框，转发照旧在跑；重开一次，看到的还是
//! 同一批规则和同一批隧道。
//!
//! 一秒重画一次（[`REFRESH`]）：流量那一行是真的在动的数字，不刷就是一张
//! 死图。这条心跳跟着对话框的实体走，框一关任务就没了。
//!
//! 起停这件事写得直白些：按 Start / Stop 既动隧道也动规则里的
//! [`enabled`](PortForward::enabled)，两者一起落库。因为用户按下去的意思是
//! "这条以后就该（不该）跑"，而不是"就这一次"。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IconName, Sizable as _,
    StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div, prelude::FluentBuilder as _,
    px,
};

use crate::{
    forward::{ForwardKind, ForwardRuntime, PRESETS, PortForward, format_traffic},
    host::Host,
    service::ForwardService,
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 流量那几个数字多久重读一次。一秒是"看得出在动"与"别为几个数字每帧重画
/// 一整屏"之间的那一档；隧道本身不受它影响，它只是这扇窗的刷新率。
const REFRESH: std::time::Duration = std::time::Duration::from_secs(1);

/// 这一屏里图标的尺寸。
const ICON: gpui_kit::Pixels = px(14.);

/// 标题行右端给对话框那颗关闭 `×` 让出来的宽度。
/// 它是绝对定位贴在弹层右上角的，不在标题这条 flex 里，挤不开。
const CLOSE_BUTTON_ROOM: gpui_kit::Pixels = px(28.);

/// 转发类型那一组按钮的测试探针名，顺序同 [`ForwardKind::ALL`]。
/// 用处见 [`PortForwardDialog::render_kind_group`]。
const KIND_PROBES: [&str; ForwardKind::ALL.len()] = [
    "port-forward-kind-local",
    "port-forward-kind-remote",
    "port-forward-kind-dynamic",
];

/// 那一组按钮外面那条凹槽的测试探针名。
const KIND_GROUP_PROBE: &str = "port-forward-kind-group";

/// 规则列表改完之后交回去的那一份。落库归调用方。
pub type ForwardsHandler = Rc<dyn Fn(Vec<PortForward>, &mut Window, &mut App)>;

/// 表单此刻填着的东西。新建和修改共用一份：一屏里只有一个表单的位置，
/// 两件事要填的字段又一模一样，差别只在 [`editing`](Self::editing)。
struct ForwardDraft {
    /// 正在改的是哪一条。`None` 就是在新建。
    ///
    /// 存 id 而不是索引：表单开着的时候列表可能变（另一条被删掉），
    /// 索引会指到别人身上，id 不会。
    editing: Option<SharedString>,
    kind: ForwardKind,
    name: Entity<InputState>,
    bind: Entity<InputState>,
    listen_port: Entity<InputState>,
    target_host: Entity<InputState>,
    target_port: Entity<InputState>,
}

/// 端口转发对话框的正文。
pub struct PortForwardDialog {
    host: Host,
    service: ForwardService,
    /// 这台主机上的规则。对话框里改的是这一份，改完交回调用方落库。
    rules: Vec<PortForward>,
    on_change: ForwardsHandler,
    /// 新建/修改表单。`None` 时那一块不画（原型里 "Add Forward" 之前的样子）。
    draft: Option<ForwardDraft>,
    /// 最近一次 "Test Socket" 的结果，挂在被测的那条规则下面。
    /// 它是一句一次性的话，不进任何持久状态。
    test_result: Option<(SharedString, Result<SharedString, SharedString>)>,
    /// 刚复制过 CLI 的那条规则。按钮上的图标换成对勾，下一次心跳清掉。
    copied: Option<SharedString>,
    /// 一秒一跳的重画心跳。丢掉它就停了（对话框关掉时自然发生）。
    _refresh: Task<()>,
}

impl PortForwardDialog {
    /// 弹出对话框。
    ///
    /// `rules` 是这台主机现在的规则（调用方那一份的拷贝），`service` 是这条
    /// 会话的转发服务。改完的规则经 `on_change` 交回去 —— 这个框不碰数据库。
    pub fn open(
        host: Host,
        service: ForwardService,
        rules: Vec<PortForward>,
        on_change: ForwardsHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let dialog = cx.new(|cx| Self::new(host.clone(), service, rules, on_change, cx));
        let header_host = host.clone();
        let header_dialog = dialog.clone();

        window.open_dialog(cx, move |modal, _, cx| {
            let dialog = header_dialog.clone();

            modal
                .w(px(680.))
                .title(render_header(&header_host, &dialog, cx))
                .child(dialog.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette::idle(cx))
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(SharedString::from(t!("port_forward.shortcut_hint"))),
                        )
                        .child(
                            Button::new("port-forward-close")
                                .ghost()
                                .label(t!("port_forward.close"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        ),
                )
        });
    }

    fn new(
        host: Host,
        service: ForwardService,
        rules: Vec<PortForward>,
        on_change: ForwardsHandler,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            host,
            service,
            rules,
            on_change,
            draft: None,
            test_result: None,
            copied: None,
            _refresh: cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(REFRESH).await;
                    let Ok(()) = this.update(cx, |this, cx| {
                        // 复制提示是一次性的：下一跳就该恢复成复制图标。
                        this.copied = None;
                        cx.notify();
                    }) else {
                        return;
                    };
                }
            }),
        }
    }

    fn active_count(&self) -> usize {
        self.service.active_count()
    }

    /// 规则改了：先记在自己这一份上，再交给调用方落库。
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rules = self.rules.clone();
        let handler = self.on_change.clone();
        handler(rules, window, cx);
        cx.notify();
    }

    // ─── 起停与增删 ─────────────────────────────────────────────────────────

    /// 起或停一条规则。
    ///
    /// 规则里的 `enabled` 跟着一起翻并落库：按 Stop 的意思是"以后别自己起来"，
    /// 而不是"这一次先停下"。落库放在动隧道之前 —— 起隧道要一次往返，而"我按
    /// 过了"这件事应当立刻看得见。
    fn toggle(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rule) = self.rules.iter().find(|rule| rule.id == id).cloned() else {
            return;
        };
        // 按下去的意思由**实况**决定，不由规则里的 `enabled` 决定：那两者可能
        // 不一致（自动起的时候端口被占，规则说要跑而隧道并没有起来），那时候
        // 按钮上写的是 Start，按下去就该是起。
        let start = !self.service.snapshot().contains_key(&id);
        if let Some(stored) = self.rules.iter_mut().find(|stored| stored.id == id) {
            stored.enabled = start;
        }
        self.test_result = None;
        self.commit(window, cx);

        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = match start {
                true => service.start(&rule).await.map(|_| ()),
                false => {
                    service.stop(&rule.id).await;
                    Ok(())
                }
            };
            this.update_in(cx, |_, window, cx| {
                if let Err(error) = outcome {
                    window.push_notification(
                        t!(
                            "port_forward.start_failed",
                            name => rule.name,
                            error => format!("{error:#}"),
                        ),
                        cx,
                    );
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 删掉一条规则。在跑的先停掉 —— 规则没了而监听还占着端口是最难查的那种
    /// 残留：界面上一条都看不见，端口却怎么也绑不上。
    fn remove(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        // 正改着的那一条被删了：表单也得收起来，否则按下保存会把刚删掉的那条
        // 又添回去 —— 那是"删除没生效"的观感。
        if self.is_editing(&id) {
            self.draft = None;
        }
        self.rules.retain(|rule| rule.id != id);
        self.test_result = None;
        self.commit(window, cx);

        let service = self.service.clone();
        cx.spawn(async move |this, cx| {
            service.stop(&id).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// 打开新建表单。
    ///
    /// 已经开着一张**新建**表单就只是把它留在那儿 —— 再点一次"新增转发"不该
    /// 把填了一半的东西冲掉。开着的是**修改**表单时换成新建：那时候用户按下
    /// "新增转发"的意思没有第二种解释。
    fn open_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .draft
            .as_ref()
            .is_some_and(|draft| draft.editing.is_none())
        {
            return;
        }
        let draft = ForwardDraft {
            editing: None,
            kind: ForwardKind::Local,
            name: cx.new(|cx| {
                InputState::new(window, cx).placeholder(t!("port_forward.name_placeholder"))
            }),
            bind: cx
                .new(|cx| InputState::new(window, cx).default_value(crate::forward::DEFAULT_BIND)),
            listen_port: cx.new(|cx| InputState::new(window, cx).placeholder("15432")),
            target_host: cx
                .new(|cx| InputState::new(window, cx).default_value(crate::forward::DEFAULT_BIND)),
            target_port: cx.new(|cx| InputState::new(window, cx).placeholder("5432")),
        };
        self.set_draft(draft, window, cx);
    }

    /// 改一条已有的规则：把它现在的值填进同一张表单。
    ///
    /// **在跑的改不了**。改完的规则和跑着的隧道会对不上 —— 端口从 15432 改成
    /// 15433 之后，列表里写着 15433，真正听着的还是 15432，那种错要等到客户端
    /// 连不上才发现。所以先停再改：卡片上那颗"修改"在这条跑着的时候本来就是
    /// 灰的，这里再挡一道，挡的是表单开着期间它被别处起起来的那种情况
    /// （重连时按 `enabled` 自动起）。
    fn start_edit(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rule) = self.rules.iter().find(|rule| rule.id == id).cloned() else {
            return;
        };
        if self.service.snapshot().contains_key(&id) {
            return;
        }
        let draft = ForwardDraft {
            editing: Some(rule.id.clone()),
            kind: rule.kind,
            name: cx.new(|cx| InputState::new(window, cx).default_value(rule.name.clone())),
            bind: cx.new(|cx| InputState::new(window, cx).default_value(rule.bind_address.clone())),
            listen_port: cx
                .new(|cx| InputState::new(window, cx).default_value(rule.listen_port.to_string())),
            target_host: cx
                .new(|cx| InputState::new(window, cx).default_value(rule.target_host.clone())),
            target_port: cx
                .new(|cx| InputState::new(window, cx).default_value(rule.target_port.to_string())),
        };
        self.test_result = None;
        self.set_draft(draft, window, cx);
    }

    /// 把一张表单放上去，并把回车接起来。
    fn set_draft(&mut self, draft: ForwardDraft, window: &mut Window, cx: &mut Context<Self>) {
        self.draft = Some(draft);
        // 名字与两个端口框都接回车：填完直接敲回车就存下并起来。
        if let Some(draft) = &self.draft {
            for input in [&draft.name, &draft.listen_port, &draft.target_port] {
                cx.subscribe_in(
                    input,
                    window,
                    |this, _, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => this.save_draft(window, cx),
                        _ => cx.notify(),
                    },
                )
                .detach();
            }
        }
        cx.notify();
    }

    fn close_draft(&mut self, cx: &mut Context<Self>) {
        self.draft = None;
        cx.notify();
    }

    /// 这条规则此刻是不是正被表单改着。卡片要据此换个样子，
    /// 它的 Start 也要按不动 —— 一条正在改的规则不该同时起来。
    fn is_editing(&self, id: &SharedString) -> bool {
        self.draft
            .as_ref()
            .and_then(|draft| draft.editing.as_ref())
            .is_some_and(|editing| editing == id)
    }

    /// 把预设填进表单。名字也一起填 —— 存之前还能改，但多数时候不用改。
    fn apply_preset(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preset) = PRESETS.get(at) else {
            return;
        };
        let Some(draft) = &self.draft else {
            return;
        };
        draft
            .name
            .update(cx, |state, cx| state.set_value(preset.label, window, cx));
        draft.listen_port.update(cx, |state, cx| {
            state.set_value(preset.listen_port.to_string(), window, cx)
        });
        draft.target_port.update(cx, |state, cx| {
            state.set_value(preset.target_port.to_string(), window, cx)
        });
        if let Some(draft) = &mut self.draft {
            draft.kind = preset.kind;
        }
        cx.notify();
    }

    /// 读表单，组一条规则出来。填得不对就是一句给用户看的话。
    ///
    /// 改的时候从**原来那一条**改起而不是重新造一条：id 得留住（隧道、界面
    /// 上的每一处都按它对齐），`enabled` 也一样。
    fn build_draft(&self, cx: &App) -> Result<PortForward, SharedString> {
        let Some(draft) = &self.draft else {
            return Err(t!("port_forward.form_closed").into());
        };
        let port = |input: &Entity<InputState>| -> u16 {
            input.read(cx).value().trim().parse().unwrap_or(0)
        };

        let mut rule = match draft
            .editing
            .as_ref()
            .and_then(|id| self.rules.iter().find(|rule| &rule.id == id))
        {
            Some(existing) => {
                let mut rule = existing.clone();
                rule.kind = draft.kind;
                rule
            }
            None => PortForward::new(draft.kind),
        };
        rule.name = draft.name.read(cx).value().trim().to_owned().into();
        rule.bind_address = draft.bind.read(cx).value().trim().to_owned().into();
        rule.listen_port = port(&draft.listen_port);
        rule.target_host = draft.target_host.read(cx).value().trim().to_owned().into();
        rule.target_port = port(&draft.target_port);
        // 动态转发没有目标，但规则里那两格总得有个值；填成监听地址本身，
        // 这样把它改回 `-L` 的时候不会看到两个空框。
        if !draft.kind.needs_target() {
            rule.target_host = rule.bind_address.clone();
            rule.target_port = rule.listen_port;
        }
        rule.description = match PRESETS.iter().find(|preset| preset.kind == rule.kind) {
            Some(preset) if preset.listen_port == rule.listen_port => preset.description(),
            _ => SharedString::default(),
        };

        rule.check()?;
        // 同一个本机端口上不能有两条规则：第二条一定绑不上，而失败要等到起的
        // 时候才知道 —— 那时候用户已经以为存好了。
        // 改的时候要把自己排除掉，不然"只改个名字"会被自己挡下来。
        let clash = self.rules.iter().any(|existing| {
            existing.id != rule.id
                && existing.kind.listens_locally() == rule.kind.listens_locally()
                && existing.listen_port == rule.listen_port
                && existing.bind_address == rule.bind_address
        });
        if clash {
            return Err(SharedString::from(t!(
                "port_forward.port_taken",
                endpoint => rule.listen_endpoint(),
            )));
        }
        Ok(rule)
    }

    /// 把表单组出来的这一条放进列表。
    ///
    /// 新建是追加，修改是**原地替换**同一个 id 那一条 —— 顺序不动，一条规则
    /// 不会因为改了个名字就跳到列表末尾。`enabled` 一律翻成真：存下来接着就要
    /// 起，那么"以后也该自己起"，而改之前按的那次 Stop 把它翻成了假。
    fn store_rule(&mut self, mut rule: PortForward) -> PortForward {
        rule.enabled = true;
        match self.rules.iter().position(|stored| stored.id == rule.id) {
            Some(at) => self.rules[at] = rule.clone(),
            None => self.rules.push(rule.clone()),
        }
        rule
    }

    /// 存下表单里那一条并立刻起起来（原型里那颗 "Save & Enable"）。
    ///
    /// 新建与修改走的是同一条路，差别只在[放进列表](Self::store_rule)那一步。
    /// 两种情形都"存下并起来"：改之前得先按 Stop，存下来再起回去，
    /// 用户按这颗按钮想要的就是"改完接着用"。
    fn save_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 表单开着的这段时间里它被起起来了（重连时按 `enabled` 自动起）：
        // 这时候落库会让规则和跑着的隧道对不上，说清楚，别默默改。
        let editing = self.draft.as_ref().and_then(|draft| draft.editing.clone());
        if let Some(id) = &editing
            && self.service.snapshot().contains_key(id)
        {
            window.push_notification(t!("port_forward.running_stop_first"), cx);
            return;
        }

        let rule = match self.build_draft(cx) {
            Ok(rule) => rule,
            Err(message) => {
                window.push_notification(message.to_string(), cx);
                return;
            }
        };
        let rule = self.store_rule(rule);
        self.draft = None;
        self.commit(window, cx);

        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.start(&rule).await;
            this.update_in(cx, |this, window, cx| {
                if let Err(error) = outcome {
                    // 起不来的规则留在列表里，但标成"别自动起"——
                    // 下次开会话不该再撞一次同一个端口。
                    if let Some(stored) = this.rules.iter_mut().find(|s| s.id == rule.id) {
                        stored.enabled = false;
                    }
                    this.commit(window, cx);
                    window.push_notification(
                        t!(
                            "port_forward.start_failed",
                            name => rule.name,
                            error => format!("{error:#}"),
                        ),
                        cx,
                    );
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ─── 单条规则上的三个动作 ───────────────────────────────────────────────

    fn copy_cli(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let Some(rule) = self.rules.iter().find(|rule| rule.id == id) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(rule.ssh_command(&self.host)));
        self.copied = Some(id);
        cx.notify();
    }

    fn test_socket(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rule) = self.rules.iter().find(|rule| rule.id == id).cloned() else {
            return;
        };
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.test(&rule).await;
            this.update(cx, |this, cx| {
                this.test_result = Some((
                    rule.id.clone(),
                    match outcome {
                        Ok(message) => Ok(SharedString::from(message)),
                        Err(error) => Err(SharedString::from(format!("{error:#}"))),
                    },
                ));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_in_browser(&self, id: &SharedString, cx: &mut Context<Self>) {
        let Some(url) = self
            .rules
            .iter()
            .find(|rule| &rule.id == id)
            .and_then(|rule| rule.browser_url())
        else {
            return;
        };
        cx.open_url(&url);
    }

    // ─── 渲染 ───────────────────────────────────────────────────────────────

    /// 新建/修改表单那一块（原型里蓝框围起来的那一段）。
    ///
    /// 两件事共用这一块，但边框、标题、图标与保存按钮上的字都跟着换：
    /// 改一条已有的规则和添一条新的，后果差得远 —— 一眼要能看出自己在干哪一件。
    /// 改的时候用琥珀色，被改的那张卡片也是同一个颜色（见
    /// [`render_rule`](Self::render_rule)），两处对得上。
    fn render_draft(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let draft = self.draft.as_ref()?;
        let kind = draft.kind;
        let editing = draft.editing.is_some();
        let accent = match editing {
            true => palette::amber(),
            false => palette::blue(),
        };

        Some(
            v_flex()
                .gap_3()
                .p_3()
                .rounded(cx.theme().radius)
                .bg(palette::surface(cx))
                .border_1()
                .border_color(accent.opacity(0.6))
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                .text_xs()
                                .font_semibold()
                                .child(
                                    match editing {
                                        true => Icon::new(AppIcon::Pencil),
                                        false => Icon::new(IconName::Plus),
                                    }
                                    .with_size(ICON)
                                    .text_color(
                                        match editing {
                                            true => accent,
                                            false => palette::teal(),
                                        },
                                    ),
                                )
                                .child(SharedString::from(match editing {
                                    true => t!("port_forward.draft_title_edit"),
                                    false => t!("port_forward.draft_title_new"),
                                })),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(palette::idle(cx))
                                        .child(SharedString::from(t!("port_forward.presets"))),
                                )
                                .children(PRESETS.iter().enumerate().map(|(ix, preset)| {
                                    Button::new(("port-forward-preset", ix))
                                        .ghost()
                                        .xsmall()
                                        .label(preset.label)
                                        .text_color(palette::coral())
                                        .tooltip(preset.description())
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.apply_preset(ix, window, cx)
                                        }))
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_start()
                        .child(div().flex_grow(1.).min_w_0().child(field(
                            t!("port_forward.field_name").into(),
                            Input::new(&draft.name).small().w_full(),
                            cx,
                        )))
                        .child(div().flex_grow(1.).min_w_0().child(field(
                            t!("port_forward.field_kind").into(),
                            self.render_kind_group(cx),
                            cx,
                        ))),
                )
                .child(render_kind_help(kind, cx))
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_start()
                        .child(
                            div().flex_grow(1.).min_w_0().child(field(
                                t!("port_forward.field_bind").into(),
                                Input::new(&draft.bind)
                                    .small()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .w_full(),
                                cx,
                            )),
                        )
                        .child(
                            div().flex_grow(1.).min_w_0().child(field(
                                kind.listen_label(),
                                Input::new(&draft.listen_port)
                                    .small()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .w_full(),
                                cx,
                            )),
                        )
                        .when(kind.needs_target(), |row| {
                            row.child(
                                div().flex_grow(2.).min_w_0().child(field(
                                    t!("port_forward.field_target").into(),
                                    h_flex()
                                        .w_full()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            div().flex_1().min_w_0().child(
                                                Input::new(&draft.target_host)
                                                    .small()
                                                    .font_family(
                                                        cx.theme().mono_font_family.clone(),
                                                    )
                                                    .w_full(),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(":"),
                                        )
                                        .child(
                                            div().w(px(84.)).child(
                                                Input::new(&draft.target_port)
                                                    .small()
                                                    .font_family(
                                                        cx.theme().mono_font_family.clone(),
                                                    )
                                                    .w_full(),
                                            ),
                                        ),
                                    cx,
                                )),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("port-forward-draft-cancel")
                                .ghost()
                                .small()
                                .label(t!("port_forward.cancel"))
                                .on_click(cx.listener(|this, _, _, cx| this.close_draft(cx))),
                        )
                        .child(
                            Button::new("port-forward-draft-save")
                                .primary()
                                .small()
                                .label(match editing {
                                    true => t!("port_forward.save_edit"),
                                    false => t!("port_forward.save_new"),
                                })
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save_draft(window, cx)),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// 三档转发那一组按钮。和分屏那一组一样是单选组，不是三个开关。
    ///
    /// 每一颗按钮外面套一层 `flex_1` 的格子，而不是给按钮本身 `w_full()`：
    /// [`Button`] 在自己的根上无条件加了 `flex_shrink_0`（见组件库的
    /// `button.rs`），所以三颗各要 100% 宽的按钮谁也不肯让，加起来 300%，
    /// 后两档被挤出可见区域 —— 看上去就是"转发类型只有本地"。让外层的格子去
    /// 三等分，按钮只管填满自己那一格。
    fn render_kind_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.draft.as_ref().map(|draft| draft.kind);
        h_flex()
            .w_full()
            .items_center()
            .gap_px()
            .p_px()
            .rounded_md()
            .bg(cx.theme().muted.opacity(0.5))
            .debug_selector(|| KIND_GROUP_PROBE.to_owned())
            .children(ForwardKind::ALL.into_iter().enumerate().map(|(ix, kind)| {
                let selected = current == Some(kind);
                let tone = palette::sky();
                let mut btn = Button::new(("port-forward-kind", ix))
                    .xsmall()
                    .w_full()
                    .label(kind.label())
                    // 选之前就该知道选的是什么：底下那条说明只讲当前这一档，
                    // 而"要不要换一档"是在鼠标停在另外两颗上时问的。
                    .tooltip(kind.tagline());
                btn = if selected {
                    btn.custom(
                        ButtonCustomVariant::new(cx)
                            .color(cx.theme().background)
                            .foreground(tone)
                            .hover(cx.theme().background)
                            .active(cx.theme().background),
                    )
                } else {
                    btn.ghost().text_color(palette::idle(cx))
                };
                div()
                    .flex_1()
                    .min_w_0()
                    // 三档都得留在那条凹槽里，而"跑到框外面"是画得出来却看不见的
                    // 那种错。挂个探针，测试才量得到（发布构建里这一行是空操作）。
                    .debug_selector(move || KIND_PROBES[ix].to_owned())
                    .child(btn.on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = &mut this.draft {
                            draft.kind = kind;
                            cx.notify();
                        }
                    })))
            }))
            .into_any_element()
    }

    /// 一条规则的卡片：状态行、拓扑行、动作行。
    ///
    /// `live` 是这一帧从服务读来的**一份**实况快照，不是每条规则各读一次 ——
    /// 那样每帧要为同一件事复制 N 遍同一张表。
    fn render_rule(
        &self,
        at: usize,
        live: &std::collections::HashMap<SharedString, ForwardRuntime>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rule = self.rules[at].clone();
        let runtime = live.get(&rule.id).cloned().unwrap_or_default();
        let active = runtime.active;
        let tone = kind_tone(rule.kind);
        let id = rule.id.clone();
        let editing = self.is_editing(&rule.id);

        v_flex()
            .gap_2p5()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            // 正在改的那一条描琥珀色的边，和上面那张表单同色：表单在最上面，
            // 被改的规则可能已经滚出视野，两处同色才对得上"我在改的是哪一条"。
            .map(|card| match (editing, active) {
                (true, _) => card
                    .bg(palette::surface(cx))
                    .border_color(palette::amber().opacity(0.7)),
                (false, true) => card
                    .bg(palette::surface(cx))
                    .border_color(palette::blue().opacity(0.5)),
                (false, false) => card
                    .bg(cx.theme().background)
                    .border_color(cx.theme().border),
            })
            .child(self.render_rule_head(at, &rule, active, tone, cx))
            .child(self.render_topology(&rule, &runtime, tone, cx))
            .child(self.render_rule_actions(at, &rule, &runtime, cx))
            .children(self.render_test_result(&id, cx))
            .into_any_element()
    }

    /// 状态点、名字、类型徽章、起停与删除。
    ///
    /// 元素 id 用行号（`at`）而不是规则 id：`ElementId` 只收得下整数与静态字符串，
    /// 而规则 id 是运行期拼出来的。行号在这一帧里是唯一的，够用。
    fn render_rule_head(
        &self,
        at: usize,
        rule: &PortForward,
        active: bool,
        tone: Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = rule.id.clone();
        let edit_id = rule.id.clone();
        let remove_id = rule.id.clone();
        let editing = self.is_editing(&rule.id);

        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(10.))
                            .rounded_full()
                            .bg(match active {
                                true => palette::green(),
                                false => palette::idle(cx),
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(rule.name.clone()),
                    )
                    .child({
                        // 徽章上写的是 `LOCAL FORWARD` 这样的行话。鼠标停上去
                        // 给一句人话 —— 已经存下来的规则也该说得清它在干什么，
                        // 而不是只有新建的时候才有说明。
                        let tagline = rule.kind.tagline();
                        div()
                            .id(("port-forward-badge", at))
                            .flex_shrink_0()
                            .px_1p5()
                            .rounded_sm()
                            .bg(tone.opacity(0.2))
                            .border_1()
                            .border_color(tone.opacity(0.4))
                            .text_color(tone)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(9.))
                            .child(rule.kind.badge())
                            .tooltip(move |window, cx| {
                                Tooltip::new(tagline.clone()).build(window, cx)
                            })
                    }),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1p5()
                    .child({
                        let green = palette::green();
                        let mut btn = Button::new(("port-forward-toggle", at))
                            .xsmall()
                            .icon(match active {
                                true => Icon::new(AppIcon::Square).with_size(px(11.)),
                                false => Icon::new(IconName::Play).with_size(px(11.)),
                            })
                            .label(if active { "Stop" } else { "Start" });
                        btn = if active {
                            btn.custom(
                                ButtonCustomVariant::new(cx)
                                    .color(green.opacity(0.2))
                                    .foreground(green)
                                    .hover(green.opacity(0.3))
                                    .active(green.opacity(0.35)),
                            )
                            .border_1()
                            .border_color(green.opacity(0.5))
                        } else {
                            btn.secondary().text_color(palette::idle(cx))
                        };
                        // 正在改的这一条起不得：起来了就再也存不下改动
                        // （见 [`save_draft`](Self::save_draft) 那道拦截），
                        // 与其让用户走到那一步，不如这颗先按不动。
                        btn.disabled(editing)
                            .when(editing, |btn| {
                                btn.tooltip(t!("port_forward.editing_cannot_start"))
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.toggle(id.clone(), window, cx)
                            }))
                    })
                    .child(
                        Button::new(("port-forward-edit", at))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(AppIcon::Pencil).with_size(px(12.)).text_color(
                                match editing {
                                    true => palette::amber(),
                                    false => palette::idle(cx),
                                },
                            ))
                            // 在跑的改不了 —— 理由见 [`start_edit`](Self::start_edit)。
                            // 按钮直接灰掉，而不是按下去再弹一句话：能不能改是
                            // 看一眼就该知道的事。
                            .disabled(active)
                            .tooltip(match active {
                                true => t!("port_forward.stop_before_edit"),
                                false => t!("port_forward.edit_rule"),
                            })
                            .accessibility_id(t!("port_forward.edit_rule"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_edit(edit_id.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new(("port-forward-remove", at))
                            .ghost()
                            .xsmall()
                            .icon(
                                Icon::new(AppIcon::Trash)
                                    .with_size(px(12.))
                                    .text_color(palette::idle(cx)),
                            )
                            .tooltip(t!("port_forward.delete_rule"))
                            .accessibility_id(t!("port_forward.delete_rule"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove(remove_id.clone(), window, cx)
                            })),
                    ),
            )
    }

    /// 拓扑行：本机一端 → 管子 → 对端一端。
    ///
    /// 端口显示的是**真正绑到的那个**（规则里写 0 时两者不同），因为这一行
    /// 回答的是"我该连哪儿"。
    fn render_topology(
        &self,
        rule: &PortForward,
        runtime: &ForwardRuntime,
        tone: Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let listen_port = match runtime.active {
            true => runtime.listen_port,
            false => rule.listen_port,
        };
        // 左边画的永远是**监听那一端**。`-L` / `-D` 监听在本机（笔记本图标），
        // `-R` 监听在对端（机架图标）—— 两个图标一换，方向就不必再靠读字看懂。
        let listens_locally = rule.kind.listens_locally();

        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .p_2()
            .rounded(cx.theme().radius)
            .bg(palette::recessed(cx))
            .border_1()
            .border_color(cx.theme().border)
            .text_xs()
            .font_family(cx.theme().mono_font_family.clone())
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_1p5()
                    .child(
                        Icon::new(if listens_locally {
                            AppIcon::Laptop
                        } else {
                            AppIcon::Server
                        })
                        .with_size(px(13.))
                        .text_color(palette::sky()),
                    )
                    .child(
                        div()
                            .text_color(palette::idle(cx))
                            .child(format!("{}:", rule.bind_address)),
                    )
                    .child(
                        div()
                            .font_semibold()
                            .text_color(palette::teal())
                            .child(listen_port.to_string()),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .text_color(palette::idle(cx))
                    .child(div().child(rule.kind.pipe()))
                    .child(
                        Icon::new(IconName::ArrowRight)
                            .with_size(px(13.))
                            .text_color(tone),
                    ),
            )
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_1p5()
                    .child(
                        Icon::new(if listens_locally {
                            AppIcon::Server
                        } else {
                            AppIcon::Laptop
                        })
                        .with_size(px(13.))
                        .text_color(palette::coral()),
                    )
                    .map(|side| match rule.target_endpoint() {
                        Some(_) => side
                            .child(
                                div()
                                    .text_color(palette::idle(cx))
                                    .child(format!("{}:", rule.target_host)),
                            )
                            .child(
                                div()
                                    .font_semibold()
                                    .text_color(palette::coral())
                                    .child(rule.target_port.to_string()),
                            ),
                        None => side.child(
                            div()
                                .text_color(palette::magenta())
                                .child(SharedString::from(t!("port_forward.dynamic_target"))),
                        ),
                    }),
            )
    }

    /// 动作行：流量、在浏览器里打开、测试端口、复制 CLI。
    fn render_rule_actions(
        &self,
        at: usize,
        rule: &PortForward,
        runtime: &ForwardRuntime,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = rule.id.clone();
        let test_id = rule.id.clone();
        let browser_id = rule.id.clone();
        let copied = self.copied.as_ref() == Some(&rule.id);
        let can_browse = runtime.active && rule.browser_url().is_some();

        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .text_xs()
            .text_color(palette::idle(cx))
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_3()
                    .child(div().child(SharedString::from(t!(
                        "port_forward.traffic",
                        amount => format_traffic(runtime.bytes),
                    ))))
                    .when(runtime.connections > 0, |row| {
                        row.child(div().child(SharedString::from(t!(
                            "port_forward.connections",
                            count => runtime.connections,
                        ))))
                    })
                    .when(can_browse, |row| {
                        row.child(
                            Button::new(("port-forward-browser", at))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::ExternalLink).with_size(px(11.)))
                                .label(t!("port_forward.open_in_browser"))
                                .text_color(palette::blue())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_in_browser(&browser_id, cx)
                                })),
                        )
                    }),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1p5()
                    .child(
                        Button::new(("port-forward-test", at))
                            .secondary()
                            .xsmall()
                            .label(t!("port_forward.test_port"))
                            .tooltip(t!("port_forward.test_port_tooltip"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.test_socket(test_id.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new(("port-forward-copy", at))
                            .secondary()
                            .xsmall()
                            .icon(
                                Icon::new(match copied {
                                    true => IconName::Check,
                                    false => IconName::Copy,
                                })
                                .with_size(px(11.))
                                .text_color(match copied {
                                    true => palette::green(),
                                    false => palette::idle(cx),
                                }),
                            )
                            .label(t!("port_forward.copy_cli"))
                            .tooltip(t!("port_forward.copy_cli_tooltip"))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.copy_cli(id.clone(), cx)),
                            ),
                    ),
            )
    }

    /// "测试端口"的结果，挂在被测的那条下面。
    fn render_test_result(&self, id: &SharedString, cx: &App) -> Option<AnyElement> {
        let (tested, outcome) = self.test_result.as_ref()?;
        if tested != id {
            return None;
        }
        let (tone, icon, message) = match outcome {
            Ok(message) => (palette::green(), IconName::CircleCheck, message.clone()),
            Err(error) => (cx.theme().danger, IconName::TriangleAlert, error.clone()),
        };
        Some(
            h_flex()
                .w_full()
                .items_center()
                .gap_1p5()
                .p_1p5()
                .rounded(cx.theme().radius)
                .bg(tone.opacity(0.15))
                .border_1()
                .border_color(tone.opacity(0.4))
                .text_xs()
                .text_color(tone)
                .child(Icon::new(icon).with_size(px(13.)).flex_shrink_0())
                .child(div().min_w_0().child(message))
                .into_any_element(),
        )
    }

    /// 一条规则都没有时的空状态。
    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .w_full()
            .items_center()
            .gap_2()
            .p_8()
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx).opacity(0.5))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                Icon::new(IconName::Network)
                    .large()
                    .text_color(palette::idle(cx)),
            )
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .text_color(cx.theme().foreground)
                    .child(SharedString::from(t!("port_forward.empty_title"))),
            )
            .child(
                div()
                    .max_w(px(420.))
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("port_forward.empty_body"))),
            )
            .child(render_kind_legend(cx))
            .child(
                Button::new("port-forward-empty-add")
                    .primary()
                    .small()
                    .icon(Icon::new(IconName::Plus).with_size(ICON))
                    .label(t!("port_forward.empty_add"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_draft(window, cx))),
            )
            .into_any_element()
    }
}

impl Render for PortForwardDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let live = self.service.snapshot();
        let rules: Vec<AnyElement> = (0..self.rules.len())
            .map(|at| self.render_rule(at, &live, cx))
            .collect();
        let draft = self.render_draft(cx);
        let empty = rules.is_empty() && draft.is_none();

        v_flex()
            .gap_3()
            .pb_1()
            .children(draft)
            .when(empty, |body| body.child(self.render_empty(cx)))
            .children(rules)
    }
}

/// 标题行：左边是图标、标题、活跃计数与这条会话是谁，右边是"新增转发"。
///
/// 活跃计数每帧从服务读一次而不是记在自己身上：隧道可能被别处停掉
/// （对话框关着的时候会话断了），记一份就会和实况对不上。
///
/// 右边留出一段空位：对话框自己那颗关闭的 `×` 是绝对定位贴在右上角的
/// （见 `gpui_kit::component::dialog`），不留位置的话"新增转发"会压在它下面。
fn render_header(host: &Host, dialog: &Entity<PortForwardDialog>, cx: &App) -> AnyElement {
    let active = dialog.read(cx).active_count();
    let teal = palette::teal();
    let blue = palette::blue();
    let add_variant = ButtonCustomVariant::new(cx)
        .color(blue)
        .foreground(palette::on_accent())
        .hover(blue.lighten(0.08))
        .active(blue.darken(0.08));
    let add = dialog.clone();

    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_2()
        .pr(CLOSE_BUTTON_ROOM)
        .child(
            h_flex()
                .min_w_0()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .flex_shrink_0()
                        .size(px(32.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(cx.theme().radius)
                        .bg(teal.opacity(0.15))
                        .border_1()
                        .border_color(teal.opacity(0.3))
                        .text_color(teal)
                        .child(Icon::new(IconName::Network).with_size(px(16.))),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .gap(px(2.))
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .child(SharedString::from(t!("port_forward.title"))),
                                )
                                .child({
                                    let tone = match active {
                                        0 => palette::idle(cx),
                                        _ => palette::green(),
                                    };
                                    h_flex()
                                        .flex_shrink_0()
                                        .items_center()
                                        .gap_1()
                                        .px_2()
                                        .rounded_full()
                                        .bg(tone.opacity(0.2))
                                        .border_1()
                                        .border_color(tone.opacity(0.4))
                                        .text_color(tone)
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(11.))
                                        .child(div().size(px(6.)).rounded_full().bg(tone))
                                        .child(SharedString::from(t!(
                                            "port_forward.running_count",
                                            count => active,
                                        )))
                                }),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(palette::idle(cx))
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(SharedString::from(t!(
                                    "port_forward.host_line",
                                    name => host.name,
                                    address => host.address(),
                                ))),
                        ),
                ),
        )
        .child(
            Button::new("port-forward-add")
                .custom(add_variant)
                .bg(blue)
                .rounded(cx.theme().radius)
                .small()
                .icon(Icon::new(IconName::Plus).with_size(ICON))
                .label(t!("port_forward.add"))
                .on_click(move |_, window, cx| {
                    add.update(cx, |this, cx| this.open_draft(window, cx));
                }),
        )
        .into_any_element()
}

/// 一个字段：标签 + 控件。和主机表单里那个是同一种排法。
fn field(label: SharedString, control: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .w_full()
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(control)
}

/// 当前选中那一档的说明，常驻在新建表单里，跟着那组按钮变。
///
/// 为什么值得占一整行：`-L` / `-R` / `-D` 只差一个字母，而字节的方向完全相反，
/// 选错的结果是"端口起来了但连不通"—— 那是最费时间的一类错。三行分工不同：
/// 第一行是一句话结论，第二行讲什么时候该选它，第三行是照抄就能用的例子。
///
/// 颜色跟着 [`kind_tone`] 走，和徽章、拓扑箭头是同一套：换一档，这一整块的
/// 色调跟着变，眼睛先看到"我换了东西"，再去读字。
fn render_kind_help(kind: ForwardKind, cx: &App) -> impl IntoElement {
    let tone = kind_tone(kind);

    h_flex()
        .w_full()
        .items_start()
        .gap_2()
        .p_2p5()
        .rounded(cx.theme().radius)
        .bg(tone.opacity(0.08))
        .border_1()
        .border_color(tone.opacity(0.35))
        .child(
            Icon::new(IconName::Info)
                .with_size(ICON)
                .flex_shrink_0()
                .text_color(tone),
        )
        .child(
            v_flex()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child(kind.tagline()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(kind.detail()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(tone)
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(kind.example()),
                ),
        )
}

/// 三档一起摆出来的对照表，给空状态用。
///
/// 一条规则都没有的时候，用户面对的问题不是"这一档是什么"而是"我该用哪一档"，
/// 那是个要横着比的问题 —— 所以这里三档并列，一行一档，而不是像表单里那样
/// 只讲选中的那一个。
fn render_kind_legend(cx: &App) -> impl IntoElement {
    v_flex()
        .w_full()
        .max_w(px(460.))
        .gap_1p5()
        .children(ForwardKind::ALL.into_iter().map(|kind| {
            let tone = kind_tone(kind);
            h_flex()
                .w_full()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(64.))
                        .px_1p5()
                        .rounded_sm()
                        .bg(tone.opacity(0.2))
                        .border_1()
                        .border_color(tone.opacity(0.4))
                        .text_color(tone)
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(10.))
                        .child(kind.label()),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(kind.tagline()),
                )
        }))
}

/// 三档各自的颜色。和原型一致：`-L` 蓝、`-R` 珊瑚、`-D` 品红。
/// 一眼要能从徽章的颜色分出这是哪一档 —— 三档的字节方向完全不同。
fn kind_tone(kind: ForwardKind) -> Hsla {
    match kind {
        ForwardKind::Local => palette::blue(),
        ForwardKind::Remote => palette::coral(),
        ForwardKind::Dynamic => palette::magenta(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forward::PortForward;
    use crate::host::{AuthMethod, MonitorIntervals};
    use crate::ssh::{SshSession, forward::ForwardRegistry};

    /// 把这一屏完整画一遍：layout → prepaint → paint 全走完。
    ///
    /// 理由与 [`crate::ui::panels::TerminalPanel`] 那个绘制测试相同：这一屏是
    /// 一整条很深的 builder 链（标题、表单、每条规则三行），而那种深度带来的
    /// 栈溢出 `cargo check` 与 clippy 都看不见，只有真的把树走一遍才掉得出来。
    /// 它不断言任何具体样式，断言的是"画得完"。
    #[gpui_kit::test]
    fn the_dialog_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw_dialog(cx, rules());
        // 再画一帧：第一帧和后续帧走的不是同一条路（元素状态、焦点都只有
        // 第二帧才读得到）。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // 新建表单那一段只有开着才画得到，三档的字段还各不相同。
        for kind in ForwardKind::ALL {
            cx.update(|window, cx| {
                dialog.update(cx, |this, cx| {
                    this.open_draft(window, cx);
                    if let Some(draft) = &mut this.draft {
                        draft.kind = kind;
                    }
                });
                window.draw(cx).clear(cx);
            });
        }

        // 修改表单是另一条分支：标题、边框、被改的那张卡片都换了样子。
        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.start_edit("fwd-local".into(), window, cx)
            });
            window.draw(cx).clear(cx);
        });
    }

    /// 三档转发都要真的落在那条凹槽里，一档都不能被挤到框外面。
    ///
    /// 这条来自一次真的翻车：三颗按钮各自 `w_full()`，而 [`Button`] 在自己的
    /// 根上无条件加了 `flex_shrink_0`，于是三颗各要 100% 宽、谁也不肯让，
    /// 加起来 300%，后两档跑到可见区域外面 —— 界面上看着就是"转发类型只有
    /// 本地"。这种错**画得完**，所以绘制测试全绿；只有量一量它们落在哪儿才
    /// 掉得出来。
    #[gpui_kit::test]
    fn every_forwarding_type_stays_inside_the_selector(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw_dialog(cx, rules());
        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| this.open_draft(window, cx));
            window.draw(cx).clear(cx);
        });

        let group = cx
            .debug_bounds(KIND_GROUP_PROBE)
            .expect("那条凹槽应该画出来了");
        for (ix, probe) in KIND_PROBES.iter().enumerate() {
            let cell = cx
                .debug_bounds(probe)
                .unwrap_or_else(|| panic!("第 {ix} 档（{probe}）压根没画出来"));
            assert!(
                cell.size.width > px(0.),
                "第 {ix} 档（{probe}）被压成了零宽"
            );
            // 半个像素的余量：布局是浮点算出来的，边界相等时不该判成越界。
            assert!(
                cell.left() >= group.left() - px(0.5) && cell.right() <= group.right() + px(0.5),
                "第 {ix} 档（{probe}）跑到了凹槽外面：它在 {:?}..{:?}，凹槽是 {:?}..{:?}",
                cell.left(),
                cell.right(),
                group.left(),
                group.right(),
            );
        }
    }

    /// 改一条规则是**原地替换**：id 不变、位置不变，列表也不该多出一条来。
    ///
    /// 这一条盯的是"改"与"新建"共用同一张表单带来的那个风险 —— 走错分支的
    /// 结果不是报错，而是列表里悄悄多出一条几乎一样的规则。
    #[gpui_kit::test]
    fn editing_a_rule_replaces_it_in_place(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw_dialog(cx, rules());
        let port = crate::db::runtime().block_on(free_port());

        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                // 改之前先按过 Stop，规则里的 enabled 是假的。
                this.rules[0].enabled = false;
                this.start_edit("fwd-local".into(), window, cx);

                let draft = this.draft.as_ref().expect("停着的规则该打得开修改表单");
                assert_eq!(draft.editing.as_deref(), Some("fwd-local"));
                // 表单里得是这条规则**现在的值**，不是一张空表。
                assert_eq!(draft.name.read(cx).value(), "PostgreSQL 5432 Local Forward");
                assert_eq!(draft.listen_port.read(cx).value(), "15432");
                assert_eq!(draft.target_host.read(cx).value(), "10.0.4.15");

                draft.name.update(cx, |state, cx| {
                    state.set_value("改过的名字", window, cx);
                });
                draft.listen_port.update(cx, |state, cx| {
                    state.set_value(port.to_string(), window, cx);
                });
            });
        });

        // 读表单 + 放进列表，也就是 `save_draft` 去掉"顺手起隧道"之后剩下的
        // 那两步。起隧道派在应用级 runtime 上，醒来的线程不是测试调度器那一条，
        // 在这儿走它只会把测试变得不确定 —— 那部分由
        // [`crate::service::ForwardService`] 自己的测试盯着。
        cx.update(|_, cx| {
            dialog.update(cx, |this, cx| {
                let rule = this.build_draft(cx).expect("这张表填得是对的");
                this.store_rule(rule);
                this.draft = None;
            });
        });

        dialog.read_with(cx, |this, _| {
            assert_eq!(this.rules.len(), 3, "改一条不该多出一条来");
            let edited = &this.rules[0];
            assert_eq!(edited.id, "fwd-local", "id 得留住，隧道靠它对齐");
            assert_eq!(edited.name, "改过的名字");
            assert_eq!(edited.listen_port, port);
            assert!(edited.enabled, "存下来就要起，那么以后也该自己起");
            assert!(this.draft.is_none(), "存完表单该收起来");
        });
    }

    /// **在跑的改不了，停下来才能改。**
    ///
    /// 改一条跑着的规则等于让列表和真正听着的那个端口对不上 —— 界面上写着新
    /// 端口，实际听着的还是旧的，那种错要等到客户端连不上才发现。
    #[gpui_kit::test]
    fn a_forward_has_to_be_stopped_before_it_can_be_edited(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw_dialog(cx, rules());
        let service = dialog.read_with(cx, |this, _| this.service.clone());

        // 真的起一条隧道：这一条问的就是"实况在跑"时的行为，光把 enabled
        // 设成真不算 —— 那只是意图。
        let port = crate::db::runtime().block_on(free_port());
        let mut running = rules().remove(0);
        running.listen_port = port;
        crate::db::runtime()
            .block_on(service.start(&running))
            .expect("假 sshd 上这条应该起得来");

        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.rules[0].listen_port = port;
                this.start_edit("fwd-local".into(), window, cx);
                assert!(this.draft.is_none(), "跑着的转发不该打得开修改表单");
            });
        });

        // 停下来，同一颗按钮就该管用了。
        crate::db::runtime().block_on(service.stop("fwd-local"));
        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.start_edit("fwd-local".into(), window, cx);
                assert!(this.draft.is_some(), "停下来之后就该改得了");
            });
        });
    }

    /// 一条规则都没有的时候画的是空状态，那是另一条分支。
    #[gpui_kit::test]
    fn the_empty_state_draws(cx: &mut gpui_kit::TestAppContext) {
        let (_dialog, cx) = draw_dialog(cx, Vec::new());
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// "测试端口"的结果条挂在被测的那一条下面，也要画得出来。
    #[gpui_kit::test]
    fn a_test_result_draws_under_its_rule(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw_dialog(cx, rules());
        for outcome in [Ok("通了".into()), Err("连不上".into())] {
            cx.update(|window, cx| {
                dialog.update(cx, |this, cx| {
                    this.test_result = Some(("fwd-local".into(), outcome.clone()));
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
        }
    }

    /// 借一个此刻空着的端口。写死端口号的测试会在别人的机器上莫名其妙地红，
    /// 与 [`crate::service::ForwardService`] 那边的同名助手同一个路子。
    async fn free_port() -> u16 {
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        probe.local_addr().unwrap().port()
    }

    fn rules() -> Vec<PortForward> {
        ForwardKind::ALL
            .into_iter()
            .map(|kind| PortForward {
                id: SharedString::from(match kind {
                    ForwardKind::Local => "fwd-local",
                    ForwardKind::Remote => "fwd-remote",
                    ForwardKind::Dynamic => "fwd-dynamic",
                }),
                name: "PostgreSQL 5432 Local Forward".into(),
                kind,
                bind_address: "127.0.0.1".into(),
                listen_port: 15432,
                target_host: "10.0.4.15".into(),
                target_port: 5432,
                enabled: true,
                description: "".into(),
            })
            .collect()
    }

    fn host() -> Host {
        Host {
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
        }
    }

    /// 把标题与正文一起挂进窗口画。
    ///
    /// 标题那一段不在 [`Render`] 里（它是交给对话框外壳的），所以单画正文
    /// 覆盖不到它 —— 而它同样是一条会读实体状态的 builder 链。
    fn draw_dialog(
        cx: &mut gpui_kit::TestAppContext,
        rules: Vec<PortForward>,
    ) -> (Entity<PortForwardDialog>, &mut gpui_kit::VisualTestContext) {
        // 真的连一次进程内的假 sshd：`ForwardService` 要一条活着的连接，而它
        // 不该为了测试变成 `Option`。那台假 sshd 就在回环上，见 `ssh::exec::fake`。
        let (handle, _) =
            crate::db::runtime().block_on(crate::ssh::exec::fake::connect("", "", true));
        let service = ForwardService::new(SshSession {
            handle,
            forwards: ForwardRegistry::default(),
        });

        cx.update(gpui_kit::init);
        let (harness, cx) = cx.add_window_view(|_, cx| {
            let dialog = cx.new(|cx| {
                PortForwardDialog::new(host(), service, rules, Rc::new(|_, _, _| {}), cx)
            });
            Harness { dialog }
        });
        let dialog = harness.read_with(cx, |harness, _| harness.dialog.clone());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (dialog, cx)
    }

    struct Harness {
        dialog: Entity<PortForwardDialog>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            v_flex()
                .size_full()
                .child(render_header(&host(), &self.dialog, cx))
                .child(self.dialog.clone())
        }
    }
}
