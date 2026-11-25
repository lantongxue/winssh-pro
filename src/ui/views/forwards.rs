//! 会话端口转发视图（原型里的 `SessionPortForwardModal`，快捷键 `Alt+P`）。
//!
//! 原先是一扇盖在窗口中央的对话框，现在常驻在右侧栏里：跟着中心区活动的
//! 那条会话走，收起右栏只是不再看它，转发照旧在跑。
//!
//! 一屏里有两件事：**改规则**和**看隧道**。它们的正本都不在这里 ——
//! 规则的正本在主机库那一行（改完经 `on_change` 交回
//! [`crate::ui::panels::TerminalPanel`] 落库），隧道的正本在
//! [`ForwardService`] 里。所以收起这一栏，转发照旧在跑；再展开，看到的还是
//! 同一批规则和同一批隧道。
//!
//! 一秒重画一次（[`REFRESH`]）：流量那一行是真的在动的数字，不刷就是一张
//! 死图。这条心跳跟着视图的实体走，标签页关掉任务才没。
//!
//! 起停这件事写得直白些：按 Start / Stop 既动隧道也动规则里的
//! [`enabled`](PortForward::enabled)，两者一起落库。因为用户按下去的意思是
//! "这条以后就该（不该）跑"，而不是"就这一次"。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    Window, div, prelude::FluentBuilder as _, px,
};

use crate::{
    forward::{ForwardKind, ForwardRuntime, PortForward, format_traffic},
    host::Host,
    service::ForwardService,
    ui::{
        Workspace,
        dialogs::{ForwardFormDialog, ForwardSavedHandler},
        theme::{AppIcon, palette},
        views::{SessionTool, right_collapse_button, session_tool_header},
    },
};
use rust_i18n::t;

/// 流量那几个数字多久重读一次。一秒是"看得出在动"与"别为几个数字每帧重画
/// 一整屏"之间的那一档；隧道本身不受它影响，它只是这一栏的刷新率。
const REFRESH: std::time::Duration = std::time::Duration::from_secs(1);

/// 这一屏里图标的尺寸。
const ICON: gpui_kit::Pixels = px(14.);

/// 规则列表改完之后交回去的那一份。落库归调用方。
pub type ForwardsHandler = Rc<dyn Fn(Vec<PortForward>, &mut Window, &mut App)>;

/// 右栏里的端口转发页：规则列表 + 隧道实况。
///
/// 新增 / 修改规则的那张表单不在这一页里 —— 它是一扇居中弹窗
/// （[`crate::ui::dialogs::ForwardFormDialog`]），按下"新增转发 / 修改"才弹，
/// 存下的那一条经回调交回本页落库并起隧道。
pub struct ForwardsView {
    host: Host,
    /// 这条会话的转发服务。会话还没连上时是 `None` —— 视图照样常驻在
    /// 终端标签页里（和监控面板同一条生命周期），连上之后由宿主
    /// [`Self::bind`] 接进来，正文从空状态换成规则列表。
    service: Option<ForwardService>,
    /// 这台主机上的规则。视图里改的是这一份，改完交回调用方落库。
    rules: Vec<PortForward>,
    on_change: ForwardsHandler,
    /// 最近一次 "Test Socket" 的结果，挂在被测的那条规则下面。
    /// 它是一句一次性的话，不进任何持久状态。
    test_result: Option<(SharedString, Result<SharedString, SharedString>)>,
    /// 刚复制过 CLI 的那条规则。按钮上的图标换成对勾，下一次心跳清掉。
    copied: Option<SharedString>,
    /// 一秒一跳的重画心跳。**只在这一页真的被右栏看着时才跑**：流量数字要动，
    /// 但没必要为一条没人看的会话每秒醒一次；收起右栏就停（丢掉任务）。
    refresh: Option<Task<()>>,
}

impl ForwardsView {
    /// 建一扇常驻在终端标签页里的转发视图。
    ///
    /// `rules` 是这台主机现在的规则（调用方那一份的拷贝），`service` 是这条
    /// 会话的转发服务（还没连上时 `None`）。改完的规则经 `on_change` 交回去
    /// —— 这一视图不碰数据库。
    pub(crate) fn new(
        host: Host,
        service: Option<ForwardService>,
        rules: Vec<PortForward>,
        on_change: ForwardsHandler,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            host,
            service,
            rules,
            on_change,
            test_result: None,
            copied: None,
            refresh: None,
        }
    }

    /// 右栏开始 / 停止看这一页。看着的时候起一秒一跳的心跳（流量数字要动），
    /// 不看就把它丢掉 —— 常驻实体不该为没人看的内容每秒醒一次。
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible == self.refresh.is_some() {
            return;
        }
        self.refresh = visible.then(|| {
            cx.spawn(async move |this, cx| {
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
            })
        });
        cx.notify();
    }

    /// 会话连上了（或重连换了连接）：把新的转发服务与主机那一行的规则接进来。
    ///
    /// 测试结果是一次性的状态，换了连接就清掉 —— 它对着的是旧连接上的隧道，
    /// 留着只会误导。
    pub(crate) fn bind(
        &mut self,
        host: Host,
        service: ForwardService,
        rules: Vec<PortForward>,
        cx: &mut Context<Self>,
    ) {
        self.host = host;
        self.service = Some(service);
        self.rules = rules;
        self.test_result = None;
        cx.notify();
    }

    /// 重拨期间摘掉旧连接：丢掉它会让仍在跑的隧道取消，视图也回到未连接态。
    pub(crate) fn unbind(&mut self, cx: &mut Context<Self>) {
        self.service = None;
        self.test_result = None;
        cx.notify();
    }

    /// 此刻的隧道实况。没连上就是空表 —— 正文据此画空状态。
    fn live(&self) -> std::collections::HashMap<SharedString, ForwardRuntime> {
        self.service
            .as_ref()
            .map(|service| service.snapshot())
            .unwrap_or_default()
    }

    fn active_count(&self) -> usize {
        self.service
            .as_ref()
            .map(|service| service.active_count())
            .unwrap_or(0)
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
        let start = !self.live().contains_key(&id);
        if let Some(stored) = self.rules.iter_mut().find(|stored| stored.id == id) {
            stored.enabled = start;
        }
        self.test_result = None;
        self.commit(window, cx);

        let Some(service) = self.service.clone() else {
            return;
        };
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
        self.rules.retain(|rule| rule.id != id);
        self.test_result = None;
        self.commit(window, cx);

        if let Some(service) = self.service.clone() {
            cx.spawn(async move |this, cx| {
                service.stop(&id).await;
                this.update(cx, |_, cx| cx.notify()).ok();
            })
            .detach();
        }
    }

    /// 弹新建表单（信息栏 / 页头上那颗"新增转发"，或空状态里那颗）。
    ///
    /// 表单是一扇居中弹窗，不再嵌在本页里；存下的那一条经回调交回
    /// [`Self::save_rule`] 落库并起隧道。
    fn open_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        let on_saved: ForwardSavedHandler = Rc::new(move |rule, window, cx| {
            view.update(cx, |this, cx| this.save_rule(rule, window, cx)).ok();
        });
        ForwardFormDialog::open(None, self.rules.clone(), on_saved, window, cx);
    }

    /// 改一条已有的规则：弹同一张表单，预填它现在的值。
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
        if self.live().contains_key(&id) {
            return;
        }
        self.test_result = None;
        let view = cx.entity().downgrade();
        let on_saved: ForwardSavedHandler = Rc::new(move |rule, window, cx| {
            view.update(cx, |this, cx| this.save_rule(rule, window, cx)).ok();
        });
        ForwardFormDialog::open(Some(rule), self.rules.clone(), on_saved, window, cx);
    }

    /// 表单存下的那一条：放进列表、落库、立刻起起来（原型里那颗 "Save & Enable"）。
    ///
    /// 新建与修改走的是同一条路，差别只在[放进列表](Self::store_rule)那一步。
    /// 两种情形都"存下并起来"：改之前得先按 Stop，存下来再起回去，
    /// 用户按这颗按钮想要的就是"改完接着用"。
    fn save_rule(&mut self, rule: PortForward, window: &mut Window, cx: &mut Context<Self>) {
        // 表单开着的这段时间里它被起起来了（重连时按 `enabled` 自动起）：
        // 这时候落库会让规则和跑着的隧道对不上，说清楚，别默默改。
        if self.live().contains_key(&rule.id) {
            window.push_notification(t!("port_forward.running_stop_first"), cx);
            return;
        }
        let rule = self.store_rule(rule);
        self.commit(window, cx);

        let Some(service) = self.service.clone() else {
            return;
        };
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
        let Some(service) = self.service.clone() else {
            return;
        };
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

        v_flex()
            .gap_2p5()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            // 在跑的那一条描蓝边、垫一层底，和停着的灰边分开 —— 余光里就看得出
            // 哪几条隧道正活着。（修改表单如今是弹窗，盖住整窗，不再需要
            // "正在改的那一条"那种琥珀标记。）
            .map(|card| match active {
                true => card
                    .bg(palette::surface(cx))
                    .border_color(palette::blue().opacity(0.5)),
                false => card
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
                        btn.on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle(id.clone(), window, cx)
                        }))
                    })
                    .child(
                        Button::new(("port-forward-edit", at))
                            .ghost()
                            .xsmall()
                            .icon(
                                Icon::new(AppIcon::Pencil)
                                    .with_size(px(12.))
                                    .text_color(palette::idle(cx)),
                            )
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

impl Render for ForwardsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = self.service.is_some();
        let live = self.live();
        let rules: Vec<AnyElement> = (0..self.rules.len())
            .map(|at| self.render_rule(at, &live, cx))
            .collect();
        let empty = rules.is_empty();
        let active = self.active_count();

        // 标题行：工具图标 + 名称 + 主机名，右端活跃计数、新增、收起。
        let tone = match active {
            0 => palette::idle(cx),
            _ => palette::green(),
        };
        let header = session_tool_header(SessionTool::Forwards, Some(self.host.name.clone()), cx)
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .rounded_full()
                    .bg(tone.opacity(0.2))
                    .text_color(tone)
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(11.))
                    .child(div().size(px(6.)).rounded_full().bg(tone))
                    .child(SharedString::from(t!(
                        "port_forward.running_count",
                        count => active,
                    ))),
            )
            .child(
                Button::new("port-forward-add")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::Plus).with_size(ICON))
                    .tooltip(t!("port_forward.add"))
                    .disabled(!connected)
                    .on_click(cx.listener(|this, _, window, cx| this.open_draft(window, cx))),
            )
            .child(
                right_collapse_button()
                    .on_click(|_, window, cx| Workspace::collapse_session_tools(window, cx)),
            );

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(header)
            // 没连上：和原先"没连接就不开这扇窗"同一条口径，画空状态而不是
            // 一屏点不动的按钮。连上之后宿主 bind 进来，这里自己换成规则列表。
            .when(!connected, |this| {
                this.child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .p_6()
                        .child(
                            Icon::new(IconName::Network)
                                .large()
                                .text_color(palette::idle(cx)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .text_center()
                                .child(SharedString::from(t!("terminal.forwards_need_ssh"))),
                        ),
                )
            })
            .when(connected, |this| {
                this.child(
                    div()
                        .id("forwards-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_2()
                        .child(
                            v_flex()
                                .gap_3()
                                .pb_1()
                                .when(empty, |body| body.child(self.render_empty(cx)))
                                .children(rules),
                        ),
                )
            })
    }
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
/// 表单弹窗（[`crate::ui::dialogs::ForwardFormDialog`]）里那条说明也用同一套。
pub(crate) fn kind_tone(kind: ForwardKind) -> Hsla {
    match kind {
        ForwardKind::Local => palette::blue(),
        ForwardKind::Remote => palette::coral(),
        ForwardKind::Dynamic => palette::magenta(),
    }
}

#[cfg(test)]
#[path = "../../tests/ui/views/forwards.rs"]
mod tests;
