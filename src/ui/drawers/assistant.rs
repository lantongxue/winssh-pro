//! 智能助手抽屉：贴着主窗口右缘的对话面板。
//!
//! 版式照设计原型的 AICopilotDrawer：标题行、消息流、输入行三段。消息流
//! 用组件库的 [`MessageScroller`]（虚拟列表 + 跟尾 + 回到最新），消息本体
//! 三种画法：用户的话靠右的实底气泡、助手的话靠左的 Markdown、命令执行卡片
//! 一块带命令与输出的边框盒。
//!
//! **审核卡片不进消息流**：它是一次要用户答复的暂态，固定画在输入行上方，
//! 批准或拒绝之后就消失 —— 拒绝会往消息流里落一张"已拒绝"的卡片，批准的
//! 那条则等命令跑完由执行结果卡片交代。
//!
//! 多会话管理都在标题行上：标题即当前对话，点开换一段、新开一段、删掉
//! 这一段。对话与消息的正本在库里（[`AssistantService`]），这里持有快照。
//!
//! 一次运行的分工：rig 侧（[`crate::assistant::agent`]）跑在 tokio 上，
//! 这里 spawn 一个泵把事件一条条搬回 gpui 线程 —— 先落库再上屏，重开应用
//! 读回来的和当时看到的是同一份。

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _,
    bubble::{Bubble, BubbleVariant},
    button::{Button, ButtonVariants as _},
    dock::DockArea,
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    message::{Message, MessageAlignment, MessageContent},
    message_scroller::{MessageScroller, MessageScrollerState},
    spinner::Spinner,
    text::TextView,
    v_flex,
};
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    Styled as _, Task, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};

use crate::assistant::{
    self, AgentEvent, ChatMessage, ChatRole, CommandStatus, Conversation, MessageBody, RunInput,
    config as assistant_config,
};
use crate::service::AssistantService;
use crate::ui::panels::TerminalPanel;
use crate::ui::theme::{AppIcon, palette};
use rust_i18n::t;

/// 抽屉宽度。固定值：对话是纵向读的，宽度跳动只会让气泡重新排版。
pub const ASSISTANT_DRAWER_WIDTH: Pixels = px(380.);

/// 标题行高度，与侧边栏的标题行同一档。
const HEADER_HEIGHT: Pixels = px(36.);

/// 带进一场运行的历史上限（用户/助手的文字消息条数）。命令卡片不进历史，
/// 模型要旧输出的话自己再查一次，比一段过期的输出可靠。
const HISTORY_LIMIT: usize = 20;

/// 命令卡片上输出最多画多少字符。工具层已经截到 8000（喂给模型的那份），
/// 画出来还要再紧一档 —— 一张几百行的卡片会把整个对话顶出屏幕。
const OUTPUT_DISPLAY_CHARS: usize = 1_200;

/// 抽屉对外的事件。
pub enum AssistantDrawerEvent {
    /// 关掉抽屉（标题行的关闭按钮）。开合归工作区管。
    Close,
}

/// 一条等着用户答复的命令。`respond` 送回 `true`（批准）或 `false`（拒绝）；
/// 直接丢掉它（停止运行时）等于让运行收口。
struct PendingApproval {
    session: SharedString,
    command: SharedString,
    respond: Option<oneshot::Sender<bool>>,
}

pub struct AssistantDrawer {
    service: AssistantService,
    /// Dock 的弱引用：按下"发送"那一刻从中心区收集已连接的会话。
    dock_area: WeakEntity<DockArea>,
    conversations: Vec<Conversation>,
    /// 当前对话。`None` = 空白的新对话，发第一条时才建档。
    active: Option<SharedString>,
    messages: Vec<ChatMessage>,
    scroller: Entity<MessageScrollerState>,
    composer: Entity<InputState>,
    /// 一场运行正在进行。进行中不许发第二条、不许切换或删除对话。
    running: bool,
    pending: Option<PendingApproval>,
    /// "停止"按钮。置位后运行在下一个模型/工具边界收口。
    cancel: Arc<AtomicBool>,
    /// 事件泵。抽屉没了它跟着没，运行侧发现通道关了自己收口。
    _run: Option<Task<()>>,
    _composer_subscription: gpui_kit::Subscription,
}

impl EventEmitter<AssistantDrawerEvent> for AssistantDrawer {}

impl AssistantDrawer {
    pub fn new(
        service: AssistantService,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(service, dock_area, window, cx);
        this.load_conversations(cx);
        this
    }

    /// 测试用：不去碰库的一具空壳，快照由测试自己摆。
    #[cfg(test)]
    fn from_snapshot(
        service: AssistantService,
        dock_area: WeakEntity<DockArea>,
        messages: Vec<ChatMessage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(service, dock_area, window, cx);
        this.set_messages(messages, cx);
        this
    }

    fn build(
        service: AssistantService,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        cx.observe(&scroller, |_, _, cx| cx.notify()).detach();
        let composer =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("assistant.composer")));
        let _composer_subscription = cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.send(window, cx);
                }
            },
        );

        Self {
            service,
            dock_area,
            conversations: Vec::new(),
            active: None,
            messages: Vec::new(),
            scroller,
            composer,
            running: false,
            pending: None,
            cancel: Arc::new(AtomicBool::new(false)),
            _run: None,
            _composer_subscription,
        }
    }

    /// 启动时把对话列表和最近那段的消息读进来。读不出来只记日志 ——
    /// 抽屉照常可用，从一段空白对话开始。
    fn load_conversations(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        cx.spawn(async move |this, cx| {
            let conversations = match service.conversations().await {
                Ok(conversations) => conversations,
                Err(error) => {
                    log::warn!("助手对话列表读不出来：{error:#}");
                    return;
                }
            };
            let latest = conversations.first().map(|chat| chat.id.clone());
            let messages = match &latest {
                Some(id) => service.messages(id).await.unwrap_or_default(),
                None => Vec::new(),
            };
            _ = this.update(cx, |this, cx| {
                this.conversations = conversations;
                this.active = latest;
                this.set_messages(messages, cx);
            });
        })
        .detach();
    }

    // ─── 消息快照 ───────────────────────────────────────────────────────────

    fn set_messages(&mut self, messages: Vec<ChatMessage>, cx: &mut Context<Self>) {
        let count = messages.len();
        self.messages = messages;
        self.scroller.update(cx, |state, cx| state.reset(count, cx));
        cx.notify();
    }

    fn push_message(&mut self, message: ChatMessage, cx: &mut Context<Self>) {
        self.messages.push(message);
        self.scroller
            .update(cx, |state, cx| _ = state.append(1, cx));
        cx.notify();
    }

    /// 界面本地的消息 id。落库那份由服务层分配，这份只喂给元素 id ——
    /// 重开应用读回来的是库里那份，两边不必对上。
    fn local_id() -> SharedString {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        format!("local-{}", COUNTER.fetch_add(1, Ordering::Relaxed)).into()
    }

    // ─── 多会话管理 ─────────────────────────────────────────────────────────

    /// 新开一段空白对话。真正的建档等第一条提问 —— 一段没说过话的对话
    /// 不值得占一行库。
    fn new_conversation(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.active = None;
        self.set_messages(Vec::new(), cx);
    }

    /// 切到另一段对话，消息从库里现读。
    fn open_conversation(&mut self, id: SharedString, cx: &mut Context<Self>) {
        if self.running || self.active.as_ref() == Some(&id) {
            return;
        }
        self.active = Some(id.clone());
        let service = self.service.clone();
        cx.spawn(async move |this, cx| {
            let messages = service.messages(&id).await.unwrap_or_default();
            _ = this.update(cx, |this, cx| {
                // 等库的这一拍里用户又切走了就别覆盖。
                if this.active.as_ref() == Some(&id) {
                    this.set_messages(messages, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// 删掉当前对话，回到空白。
    fn delete_conversation(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Some(id) = self.active.take() else {
            return;
        };
        self.conversations.retain(|chat| chat.id != id);
        self.set_messages(Vec::new(), cx);
        let service = self.service.clone();
        cx.spawn(async move |_, _| {
            if let Err(error) = service.delete_conversation(&id).await {
                log::warn!("助手对话删不掉：{error:#}");
            }
        })
        .detach();
    }

    // ─── 发送与运行 ─────────────────────────────────────────────────────────

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let prompt = self.composer.read(cx).value().trim().to_string();
        if prompt.is_empty() {
            return;
        }
        let config = assistant_config::global();
        if !config.ready() {
            window.push_notification(t!("assistant.not_configured"), cx);
            return;
        }
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));

        // 已连接的会话与对话历史都在这一刻定格。
        let sessions = self
            .dock_area
            .upgrade()
            .map(|area| TerminalPanel::connected_targets_in(&area, cx))
            .unwrap_or_default();
        let text_history: Vec<(ChatRole, String)> = self
            .messages
            .iter()
            .filter_map(|message| match &message.body {
                MessageBody::Text(text) => Some((message.role, text.to_string())),
                MessageBody::Command { .. } => None,
            })
            .collect();
        let start = text_history.len().saturating_sub(HISTORY_LIMIT);
        let history = text_history[start..].to_vec();

        self.push_message(
            ChatMessage {
                id: Self::local_id(),
                role: ChatRole::User,
                body: MessageBody::Text(prompt.clone().into()),
            },
            cx,
        );

        self.running = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let service = self.service.clone();
        let active = self.active.clone();
        let (events_tx, mut events_rx) = mpsc::unbounded();

        self._run = Some(cx.spawn_in(window, async move |this, cx| {
            // 第一条提问才建档。建不出来（库坏了）就只跑不存，界面照常。
            let conversation = match active {
                Some(id) => Some(id),
                None => match service.create_conversation(&prompt).await {
                    Ok(chat) => {
                        let id = chat.id.clone();
                        _ = this.update(cx, |this, cx| {
                            this.conversations.insert(0, chat);
                            this.active = Some(id.clone());
                            cx.notify();
                        });
                        Some(id)
                    }
                    Err(error) => {
                        log::warn!("助手对话建不了档，这一场只跑不存：{error:#}");
                        None
                    }
                },
            };
            if let Some(id) = &conversation
                && let Err(error) = service
                    .append_message(id, ChatRole::User, MessageBody::Text(prompt.clone().into()))
                    .await
            {
                log::warn!("用户消息存不下去：{error:#}");
            }

            // rig 在 tokio 上跑（reqwest 要它的 reactor），这里只当泵：
            // 事件先落库再上屏。
            let run = crate::db::runtime().spawn(assistant::run(RunInput {
                config,
                sessions,
                history,
                prompt,
                events: events_tx,
                cancel,
            }));

            while let Some(event) = events_rx.next().await {
                let (role, body) = match event {
                    AgentEvent::Text(text) => (ChatRole::Assistant, MessageBody::Text(text.into())),
                    AgentEvent::Command {
                        session,
                        command,
                        output,
                        ok,
                    } => (
                        ChatRole::Assistant,
                        MessageBody::Command {
                            command: command.into(),
                            session,
                            output: output.into(),
                            status: if ok {
                                CommandStatus::Ok
                            } else {
                                CommandStatus::Failed
                            },
                        },
                    ),
                    AgentEvent::Approval(request) => {
                        let alive = this.update(cx, |this, cx| {
                            this.pending = Some(PendingApproval {
                                session: request.session,
                                command: request.command.into(),
                                respond: Some(request.respond),
                            });
                            cx.notify();
                        });
                        if alive.is_err() {
                            return;
                        }
                        continue;
                    }
                };
                if let Some(id) = &conversation
                    && let Err(error) = service.append_message(id, role, body.clone()).await
                {
                    log::warn!("助手消息存不下去：{error:#}");
                }
                let alive = this.update(cx, |this, cx| {
                    this.push_message(
                        ChatMessage {
                            id: Self::local_id(),
                            role,
                            body,
                        },
                        cx,
                    );
                });
                if alive.is_err() {
                    return;
                }
            }

            // 通道关了 = 运行收尾。错误摆进消息流而不是弹通知：它是这段
            // 对话的一部分，用户切回来还该看得见出了什么事。
            let failure = match run.await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(format!("{error:#}")),
                Err(join_error) => Some(join_error.to_string()),
            };
            _ = this.update(cx, |this, cx| {
                this.running = false;
                this.pending = None;
                if let Some(reason) = failure {
                    this.push_message(
                        ChatMessage {
                            id: Self::local_id(),
                            role: ChatRole::Assistant,
                            body: MessageBody::Text(
                                t!("assistant.run_failed", error => reason).into(),
                            ),
                        },
                        cx,
                    );
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// 停止这一场。置位之后运行在下一个模型/工具边界收口；正等着审核的
    /// 那条命令连回执一起丢掉 —— 钩子那边把"没人答复"当停处理。
    fn stop(&mut self, cx: &mut Context<Self>) {
        if !self.running {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.pending = None;
        cx.notify();
    }

    /// 审核：批准（`true`）或拒绝（`false`）。拒绝往消息流里落一张卡片 ——
    /// 批准的那条等命令跑完由执行结果卡片交代，这里不重复记。
    fn decide(&mut self, approve: bool, cx: &mut Context<Self>) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        if let Some(respond) = pending.respond.take() {
            _ = respond.send(approve);
        }
        if !approve {
            let body = MessageBody::Command {
                command: pending.command,
                session: pending.session,
                output: SharedString::default(),
                status: CommandStatus::Denied,
            };
            if let Some(id) = self.active.clone() {
                let service = self.service.clone();
                let record = body.clone();
                cx.spawn(async move |_, _| {
                    if let Err(error) = service
                        .append_message(&id, ChatRole::Assistant, record)
                        .await
                    {
                        log::warn!("拒绝记录存不下去：{error:#}");
                    }
                })
                .detach();
            }
            self.push_message(
                ChatMessage {
                    id: Self::local_id(),
                    role: ChatRole::Assistant,
                    body,
                },
                cx,
            );
        }
        cx.notify();
    }

    // ─── 渲染 ───────────────────────────────────────────────────────────────

    /// 标题行：图标、当前对话（点开切换）、新对话、删除、关闭。
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 标题在这儿再截一档：按钮不会替里面的文字省略，太长会把右边
        // 那几颗动作按钮挤出抽屉。
        let title: SharedString = self
            .active
            .as_ref()
            .and_then(|id| self.conversations.iter().find(|chat| &chat.id == id))
            .map(|chat| {
                if chat.title.chars().count() > 14 {
                    let mut clipped: String = chat.title.chars().take(14).collect();
                    clipped.push('…');
                    clipped.into()
                } else {
                    chat.title.clone()
                }
            })
            .unwrap_or_else(|| t!("assistant.new_chat").into());
        let auto = assistant_config::global().auto_execute;

        h_flex()
            .flex_shrink_0()
            .h(HEADER_HEIGHT)
            .px_2()
            .gap_1p5()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                Icon::new(AppIcon::Sparkles)
                    .with_size(px(16.))
                    .flex_shrink_0()
                    .text_color(palette::teal()),
            )
            .child(self.render_conversation_menu(title, cx))
            // 全自动模式亮一块警示牌：命令不送审是要一眼看得见的状态。
            .when(auto, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .text_xs()
                        .bg(palette::amber().opacity(0.15))
                        .text_color(palette::amber())
                        .child(t!("assistant.auto_badge")),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("assistant-new-chat")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip(t!("assistant.new_chat"))
                    .disabled(self.running)
                    .on_click(cx.listener(|this, _, _, cx| this.new_conversation(cx))),
            )
            .child(
                Button::new("assistant-delete-chat")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(AppIcon::Trash))
                    .tooltip(t!("assistant.delete_chat"))
                    .disabled(self.running || self.active.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.delete_conversation(cx))),
            )
            .child(
                Button::new("assistant-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("assistant.close"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(AssistantDrawerEvent::Close))),
            )
    }

    /// 标题即菜单：点开列出全部对话，挑一段切过去。
    fn render_conversation_menu(
        &self,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let conversations = self.conversations.clone();
        let active = self.active.clone();
        let this = cx.entity().downgrade();

        Button::new("assistant-conversations")
            .ghost()
            .xsmall()
            .label(title)
            .icon(IconName::ChevronDown)
            .disabled(self.running)
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                let mut menu = menu.min_w(px(240.));
                if conversations.is_empty() {
                    return menu.item(PopupMenuItem::element({
                        move |_, cx| {
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("assistant.no_chats"))
                                .into_any_element()
                        }
                    }));
                }
                for chat in conversations.iter() {
                    let id = chat.id.clone();
                    let label = chat.title.clone();
                    let selected = active.as_ref() == Some(&chat.id);
                    let this = this.clone();
                    let mut item = PopupMenuItem::element(move |_, _| {
                        div()
                            .w_full()
                            .truncate()
                            .child(label.clone())
                            .into_any_element()
                    })
                    .on_click(move |_, _, cx| {
                        let id = id.clone();
                        _ = this.update(cx, |this, cx| this.open_conversation(id, cx));
                    });
                    if selected {
                        item = item.icon(Icon::new(IconName::Check).with_size(px(14.)));
                    }
                    menu = menu.item(item);
                }
                menu
            })
    }

    /// 消息流；一条消息都没有时是空状态说明。
    fn render_transcript(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.messages.is_empty() && !self.running {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .px_6()
                .child(
                    Icon::new(AppIcon::Sparkles)
                        .with_size(px(28.))
                        .text_color(palette::idle(cx)),
                )
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(t!("assistant.empty_title")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_center()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("assistant.empty_hint")),
                )
                .into_any_element();
        }

        let messages = Rc::new(self.messages.clone());
        MessageScroller::new(
            "assistant-transcript",
            self.scroller.clone(),
            move |index, _, cx| {
                messages
                    .get(index)
                    .map(|message| render_message(message, cx))
                    .unwrap_or_else(|| div().into_any_element())
            },
        )
        .with_bottom_fade(cx.theme().sidebar)
        .size_full()
        .into_any_element()
    }

    /// 审核卡片：固定在输入行上方的一块警示区。
    fn render_approval(
        &self,
        pending: &PendingApproval,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .flex_shrink_0()
            .mx_3()
            .mb_2()
            .p_2p5()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(palette::amber().opacity(0.5))
            .bg(palette::amber().opacity(0.08))
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .text_xs()
                    .text_color(palette::amber())
                    .child(Icon::new(IconName::TriangleAlert).with_size(px(14.)))
                    .child(t!("assistant.approval_title", session => pending.session.clone())),
            )
            .child(
                div()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .bg(palette::recessed(cx))
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .child(pending.command.clone()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("assistant-deny")
                            .outline()
                            .xsmall()
                            .icon(IconName::Close)
                            .label(t!("assistant.deny"))
                            .on_click(cx.listener(|this, _, _, cx| this.decide(false, cx))),
                    )
                    .child(
                        Button::new("assistant-approve")
                            .primary()
                            .xsmall()
                            .icon(IconName::Check)
                            .label(t!("assistant.approve"))
                            .on_click(cx.listener(|this, _, _, cx| this.decide(true, cx))),
                    ),
            )
    }

    /// 输入行。运行中输入框还在（可以先打下一句），发送键换成停止键。
    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(div().flex_1().min_w_0().child(Input::new(&self.composer)))
            .map(|this| {
                if self.running {
                    this.child(
                        Button::new("assistant-stop")
                            .outline()
                            .icon(IconName::Pause)
                            .tooltip(t!("assistant.stop"))
                            .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                    )
                } else {
                    this.child(
                        Button::new("assistant-send")
                            .primary()
                            .icon(IconName::ArrowUp)
                            .tooltip(t!("assistant.send"))
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    )
                }
            })
    }
}

impl Render for AssistantDrawer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("assistant-drawer")
            .flex_shrink_0()
            .w(ASSISTANT_DRAWER_WIDTH)
            .h_full()
            .border_l_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.theme().sidebar)
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().child(self.render_transcript(cx)))
            // 运行指示：泵还开着、又没有在等审核时，说明模型在想或命令在跑。
            .when(self.running && self.pending.is_none(), |this| {
                this.child(
                    h_flex()
                        .flex_shrink_0()
                        .gap_2()
                        .px_3()
                        .pb_2()
                        .items_center()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(Spinner::new().small())
                        .child(t!("assistant.thinking")),
                )
            })
            .map(|this| match self.pending.as_ref() {
                Some(pending) => this.child(self.render_approval(pending, cx)),
                None => this,
            })
            .child(self.render_composer(cx))
    }
}

/// 一条消息画成一行。独立函数：MessageScroller 的行渲染器拿不到 `Context<Self>`。
fn render_message(message: &ChatMessage, cx: &App) -> AnyElement {
    match &message.body {
        MessageBody::Text(text) => match message.role {
            ChatRole::User => Message::new()
                .alignment(MessageAlignment::End)
                .content(
                    MessageContent::new().bubble(
                        Bubble::new()
                            .with_variant(BubbleVariant::Muted)
                            .child(text.clone()),
                    ),
                )
                .into_any_element(),
            ChatRole::Assistant => div()
                .w_full()
                .min_w_0()
                .text_sm()
                .child(TextView::markdown(message.id.clone(), text.clone()))
                .into_any_element(),
        },
        MessageBody::Command {
            command,
            session,
            output,
            status,
        } => render_command_card(message, command, session, output, *status, cx),
    }
}

/// 命令执行卡片：状态行、命令、输出（有的话）。
fn render_command_card(
    message: &ChatMessage,
    command: &SharedString,
    session: &SharedString,
    output: &SharedString,
    status: CommandStatus,
    cx: &App,
) -> AnyElement {
    let (icon, tone, label): (IconName, Hsla, SharedString) = match status {
        CommandStatus::Ok => (
            IconName::CircleCheck,
            palette::green(),
            t!("assistant.status_ok").into(),
        ),
        CommandStatus::Failed => (
            IconName::CircleX,
            palette::coral(),
            t!("assistant.status_failed").into(),
        ),
        CommandStatus::Denied => (
            IconName::TriangleAlert,
            palette::amber(),
            t!("assistant.status_denied").into(),
        ),
    };

    let shown: SharedString = if output.chars().count() > OUTPUT_DISPLAY_CHARS {
        let mut clipped: String = output.chars().take(OUTPUT_DISPLAY_CHARS).collect();
        clipped.push('…');
        clipped.into()
    } else {
        output.clone()
    };

    v_flex()
        .id(SharedString::from(format!(
            "assistant-command-{}",
            message.id
        )))
        .w_full()
        .min_w_0()
        .gap_1p5()
        .p_2p5()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
        .bg(palette::surface(cx))
        .child(
            h_flex()
                .gap_1p5()
                .items_center()
                .text_xs()
                .child(Icon::new(icon).with_size(px(14.)).text_color(tone))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(cx.theme().muted_foreground)
                        .child(session.clone()),
                )
                .child(div().flex_shrink_0().text_color(tone).child(label)),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .font_family(cx.theme().mono_font_family.clone())
                .text_sm()
                .child(command.clone()),
        )
        .when(!shown.is_empty(), |this| {
            this.child(
                div()
                    .w_full()
                    .min_w_0()
                    .p_2()
                    .rounded_md()
                    .bg(palette::recessed(cx))
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(shown),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt as _;

    fn sample_messages() -> Vec<ChatMessage> {
        vec![
            ChatMessage {
                id: "m1".into(),
                role: ChatRole::User,
                body: MessageBody::Text("看看 web-01 磁盘还剩多少".into()),
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
            ChatMessage {
                id: "m4".into(),
                role: ChatRole::Assistant,
                body: MessageBody::Text("根分区还有 **28G** 可用。".into()),
            },
        ]
    }

    /// 三种消息画法齐上阵地画一整帧：气泡、Markdown、三种状态的命令卡片。
    /// 深 builder 链上的错只有真的画一遍才掉得出来。
    #[gpui_kit::test]
    fn the_drawer_draws_every_message_shape(cx: &mut gpui_kit::TestAppContext) {
        let service = AssistantService::in_memory_blocking();
        cx.update(gpui_kit::init);
        let (drawer, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("assistant-test-dock", None, window, cx));
            AssistantDrawer::from_snapshot(
                service,
                dock_area.downgrade(),
                sample_messages(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        // 第二帧走的不是同一条路：虚拟列表的量测只有这一帧才读得到。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        drawer.read_with(cx, |drawer, _| {
            assert_eq!(drawer.messages.len(), 4);
            assert!(!drawer.running);
        });
    }

    /// 审核卡片开着也要画得通，而且"拒绝"要落一张 Denied 卡片、
    /// 把 `false` 送回运行侧。
    #[gpui_kit::test]
    fn denying_an_approval_records_a_card(cx: &mut gpui_kit::TestAppContext) {
        let service = AssistantService::in_memory_blocking();
        cx.update(gpui_kit::init);
        let (drawer, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("assistant-test-dock", None, window, cx));
            AssistantDrawer::from_snapshot(service, dock_area.downgrade(), Vec::new(), window, cx)
        });

        let (respond, decision) = oneshot::channel();
        drawer.update(cx, |drawer, cx| {
            drawer.running = true;
            drawer.pending = Some(PendingApproval {
                session: "web-01".into(),
                command: "systemctl restart nginx".into(),
                respond: Some(respond),
            });
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        drawer.update(cx, |drawer, cx| drawer.decide(false, cx));
        drawer.read_with(cx, |drawer, _| {
            assert!(drawer.pending.is_none(), "答复之后卡片该收走");
            let Some(ChatMessage {
                body:
                    MessageBody::Command {
                        status: CommandStatus::Denied,
                        command,
                        ..
                    },
                ..
            }) = drawer.messages.last()
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
}
