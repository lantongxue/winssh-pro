//! 智能助手面板：Dock 里的一块对话面板，默认停在右侧 dock。
//!
//! 它是**正经的 Dock 面板**而不是自己贴在窗口右缘的一层：标签条、宽度、
//! 开合、拖拽与分屏都归 Dock，和终端、SFTP 编辑器同一套。所以这块面板
//! 能被拖到中心区当标签页、能和别的面板并排，也能整块收起来（中心区标签
//! 条右端那颗 `PanelRight`，或活动栏上的 AI 助手键）。
//!
//! 面板自己画的只有下半段：消息流与输入行。上半段那条标签条是 Dock 画的，
//! 三样东西从 [`Panel`] 的钩子递上去 —— 标题是当前对话（点开换一段，
//! [`Panel::title`]）、全自动那块警示牌（[`Panel::title_suffix`]）、
//! 新开与删除两颗按钮（[`Panel::toolbar_buttons`]）。
//!
//! 消息流用组件库的 [`MessageScroller`]（虚拟列表 + 跟尾 + 回到最新），
//! 消息本体三种画法：用户的话靠右的实底气泡、助手的话靠左的 Markdown、
//! 命令执行卡片一块带命令与输出的边框盒。
//!
//! 输入行是一块卡片：多行输入框（[`Textarea`]，跟着内容长）在上，发送/停止
//! 那颗圆键在它右下角，边框、底色与聚焦光环归卡片。回车发送、Shift+回车
//! 换行 —— 见 [`AssistantPanel::render_composer`]。
//!
//! **命令卡片默认是收起的**：一条 `cat /etc/os-release; free -h; df -hT`
//! 的输出能有几十行，摊开来会把对话本身顶出屏幕。收着的时候留一行截断的
//! 命令（跑了什么得看得见），点一下卡片头才展开全文与输出。哪几张是开着的
//! 记在 [`AssistantPanel::expanded`]（按消息 id），换对话就清空 —— 虚拟
//! 列表按量出来的高度排版，所以开合之后那一行要重新量（[`Self::remeasure`]）。
//!
//! **模型的话是流着上屏的。** 运行侧每吐一小段就来一条
//! [`AgentEvent::TextDelta`]，[`Self::stream_delta`] 把它接在正在写的那条
//! 消息后面；这一轮说完了来一条 [`AgentEvent::Text`]（规范全文），
//! [`Self::commit_stream`] 整条替换掉再落库。所以屏上先有字、库里后有记录，
//! 和命令卡片"先落库再上屏"是反的 —— 流式输出就是这个意思。半路被停掉或
//! 者出错时，说到一半的那条也会照样留下来。
//!
//! **审核卡片不进消息流**：它是一次要用户答复的暂态，固定画在输入行上方，
//! 批准或拒绝之后就消失 —— 拒绝会往消息流里落一张"已拒绝"的卡片，批准的
//! 那条则等命令跑完由执行结果卡片交代。
//!
//! **助手只操作当前打开的那条会话。** 这块面板是那格 [`CurrentSession`] 的
//! 看门人：中心区换了活动标签页（Dock 的 `LayoutChanged`）、那条会话连上或
//! 断了（[`SessionEvent::LivenessChanged`]）、以及每次发问之前，都重认一遍
//! 并换掉槽里那一条。运行侧握的是同一格，所以跑到一半切标签页也立刻生效，
//! 模型在下一次开口前就会读到新的那台机器。输入行上方那条会话名写的就是
//! 它 —— 命令会落在哪儿，用户不该靠猜。
//!
//! **"当前会话"只认中心区。** 这块面板自己也停在 Dock 里，但
//! [`TerminalPanel::active_in`] 只走中心区的布局树，所以它不会把自己
//! 或者右侧 dock 里的别的东西当成一条会话。
//!
//! 对话与消息的正本在库里（[`AssistantService`]），这里持有快照。
//!
//! 一次运行的分工：rig 侧（[`crate::assistant::agent`]）跑在 tokio 上，
//! 这里 spawn 一个泵把事件一条条搬回 gpui 线程。命令卡片是先落库再上屏，
//! 模型的话反过来（见上面那段）—— 两条路殊途同归：重开应用读回来的，和
//! 当时屏上看到的是同一份。

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    ThemeStyled as _, WindowExt as _,
    bubble::{Bubble, BubbleVariant},
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, DockArea, DockEvent, Panel, PanelEvent},
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    menu::{DropdownMenu as _, PopupMenuItem},
    message::{Message, MessageAlignment, MessageContent, MessageFooter},
    message_scroller::{MessageScroller, MessageScrollerState},
    spinner::Spinner,
    text::TextView,
    v_flex,
};
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter,
    FocusHandle, Focusable, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};

use crate::assistant::{
    self, AgentEvent, ChatMessage, ChatRole, CommandStatus, Conversation, CurrentSession,
    MessageBody, RunInput, config as assistant_config,
};
use crate::service::AssistantService;
use crate::ui::panels::{SessionEvent, TerminalPanel};
use crate::ui::theme::{AppIcon, palette};
use rust_i18n::t;

/// 右侧 dock 的初始宽度。之后由用户拖，Dock 自己记着。
pub const ASSISTANT_DOCK_WIDTH: Pixels = px(380.);

/// 标签条上那个对话名最多画多少字。按钮不会替里面的文字省略，太长会把
/// 右边那几颗动作按钮挤出标签条。
const TITLE_CHARS: usize = 14;

/// 带进一场运行的历史上限（用户/助手的文字消息条数）。命令卡片不进历史，
/// 模型要旧输出的话自己再查一次，比一段过期的输出可靠。
const HISTORY_LIMIT: usize = 20;

/// 命令卡片上输出最多画多少字符。工具层已经截到 8000（喂给模型的那份），
/// 画出来还要再紧一档 —— 一张几百行的卡片会把整个对话顶出屏幕。
const OUTPUT_DISPLAY_CHARS: usize = 1_200;

/// 换对话那颗按钮在调试边界表里的名字。标签条归 Dock 画，这块面板只把一棵
/// 元素树递上去 —— 它到底有没有落到屏幕上，只有画完一帧去问这张表才知道。
const CONVERSATION_MENU_SELECTOR: &str = "assistant-conversation-menu";

/// 复制键在调试边界表里的名字。每条消息都有一颗，同名 —— 表里只会留下最后
/// 画的那一颗，所以用它的测试摆一条消息就够了。
const COPY_SELECTOR: &str = "assistant-copy";

/// 命令卡片上那块开合区（状态行 + 命令）在调试边界表里的名字。和复制键一样
/// 每张卡片一个同名的，测试摆一张卡片就够了。
const CARD_HEAD_SELECTOR: &str = "assistant-card-head";

/// 命令卡片上那块输出在调试边界表里的名字。收着的时候它压根不画 ——
/// 查不到它正是"默认收起"的证据。
const CARD_OUTPUT_SELECTOR: &str = "assistant-card-output";

/// 输入框空着的时候占几行。一行的框看起来像个搜索栏，而这里等的是一个问题
/// —— 两行既摆得下一句话，也不至于在一块 380px 宽的面板里喧宾夺主。
const COMPOSER_MIN_ROWS: usize = 2;

/// 输入框最多长到几行，再多就在框里滚。不封顶的话，粘一段几十行的日志进来
/// 会把整条对话顶出屏幕 —— 而那正是用户粘它进来想让助手看的东西。
const COMPOSER_MAX_ROWS: usize = 8;

/// 发送/停止那颗圆键的直径。图标按 0.75 倍跟着它走（[`Button`] 自己算的）。
const SEND_BUTTON_SIZE: Pixels = px(28.);

/// 一条等着用户答复的命令。`respond` 送回 `true`（批准）或 `false`（拒绝）；
/// 直接丢掉它（停止运行时）等于让运行收口。
struct PendingApproval {
    session: SharedString,
    command: SharedString,
    respond: Option<oneshot::Sender<bool>>,
}

/// 正在一个字一个字往外写的那条助手消息。
///
/// 原文单独留一份而不是每次从消息里读回来：`SharedString` 是不可变的，
/// 追加一段就得把整条重拼一次，攒在这儿只拼一次。
struct Streaming {
    /// 它在 [`AssistantPanel::messages`] 里的下标。运行期间没有谁会删消息
    /// （切换/新建/删除对话在跑的时候都是禁着的），所以这个下标是稳的。
    index: usize,
    text: String,
}

pub struct AssistantPanel {
    service: AssistantService,
    /// Dock 的弱引用：当前是哪条会话在前面，从中心区的布局树上认。
    dock_area: WeakEntity<DockArea>,
    /// 助手唯一能操作的那台机器。运行侧握的是同一格（见模块文档）。
    session: CurrentSession,
    /// 当前那条会话所在的面板。留着它为两件事：认出"标签页真的换了"，
    /// 以及跟着它的存活变化再认一遍 —— 连上和断开都会改答案，而那两件事
    /// 不动 Dock 的布局，光听 `LayoutChanged` 听不见。
    current: Option<Entity<TerminalPanel>>,
    _liveness: Option<Subscription>,
    _layout: Option<Subscription>,
    conversations: Vec<Conversation>,
    /// 当前对话。`None` = 空白的新对话，发第一条时才建档。
    active: Option<SharedString>,
    messages: Vec<ChatMessage>,
    /// 摊开着的那几张命令卡片（按消息 id）。默认一张都不开。
    expanded: HashSet<SharedString>,
    scroller: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    /// 一场运行正在进行。进行中不许发第二条、不许切换或删除对话。
    running: bool,
    /// 正在流式写入的那条消息，没有就是这一轮还没开口（或者已经说完了）。
    streaming: Option<Streaming>,
    pending: Option<PendingApproval>,
    /// "停止"按钮。置位后运行在下一个模型/工具边界收口。
    cancel: Arc<AtomicBool>,
    /// 事件泵。面板没了它跟着没，运行侧发现通道关了自己收口。
    _run: Option<Task<()>>,
    _composer_subscription: gpui_kit::Subscription,
}

/// Dock 要面板会报 [`PanelEvent`]（关闭、缩放这类），这块面板自己不报
/// 任何一条 —— 它既不关自己也不缩放，那些都归 Dock 的标签条。
impl EventEmitter<PanelEvent> for AssistantPanel {}

impl Focusable for AssistantPanel {
    /// 交出去的是输入框的句柄：Dock 把这块面板叫到前面时，光标就该在
    /// 能打字的地方。
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }
}

impl AssistantPanel {
    pub fn new(
        service: AssistantService,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(service, dock_area, window, cx);
        this.refresh_session(window, cx);
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
        this.refresh_session(window, cx);
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
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(t!("assistant.composer"))
                // 回车发送、Shift+回车换行：聊天框的通行约定。换行得留着 ——
                // 一段贴进来的报错或者配置不该在输入框里被拍平成一行。
                .submit_on_enter(true)
                .auto_grow(COMPOSER_MIN_ROWS, COMPOSER_MAX_ROWS)
        });
        let _composer_subscription = cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &InputEvent, window, cx| {
                // Shift+回车换的那一行也报一条 PressEnter（`shift: true`），
                // 不挑一下的话每换一行就发一次。
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.send(window, cx);
                }
            },
        );

        // 中心区一动就重认当前会话。`LayoutChanged` 比"切了标签页"宽 ——
        // 拖拽、分屏、开关标签页都会来一下 —— 但重认一遍只是走一趟布局树，
        // 而漏掉任何一种都会让助手停在上一台机器上。
        let _layout = dock_area.upgrade().map(|area| {
            cx.subscribe_in(&area, window, |this, _, event: &DockEvent, window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    this.refresh_session(window, cx);
                }
            })
        });

        Self {
            service,
            dock_area,
            session: CurrentSession::default(),
            current: None,
            _liveness: None,
            _layout,
            conversations: Vec::new(),
            active: None,
            messages: Vec::new(),
            expanded: HashSet::new(),
            scroller,
            composer,
            running: false,
            streaming: None,
            pending: None,
            cancel: Arc::new(AtomicBool::new(false)),
            _run: None,
            _composer_subscription,
        }
    }

    /// 启动时把对话列表和最近那段的消息读进来。读不出来只记日志 ——
    /// 面板照常可用，从一段空白对话开始。
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

    // ─── 当前会话 ───────────────────────────────────────────────────────────

    /// 重认一遍"助手眼下能操作的是哪台机器"，答案换了就换掉那一格。
    ///
    /// 三处叫它：中心区布局一动（换标签页正是其中一种）、当前那条会话的存活
    /// 翻面、以及每次发问之前。认的是 [`TerminalPanel::active_in`] ——
    /// 和侧边栏发命令片段认的是同一条"当前会话"。
    fn refresh_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self
            .dock_area
            .upgrade()
            .and_then(|area| TerminalPanel::active_in(&area, window, cx));

        // 真的换了面板才重挂存活订阅：同一个面板反复重挂只是把上一条丢掉
        // 再建一条，白费。
        if self.current.as_ref().map(Entity::entity_id) != panel.as_ref().map(Entity::entity_id) {
            self._liveness = panel.as_ref().map(|panel| {
                cx.subscribe_in(
                    panel,
                    window,
                    |this, _, event: &SessionEvent, window, cx| {
                        if event == &SessionEvent::LivenessChanged {
                            this.refresh_session(window, cx);
                        }
                    },
                )
            });
            self.current = panel.clone();
        }

        let target = panel.and_then(|panel| panel.read(cx).session_target(cx));
        let before = self.session.get().map(|target| target.id);
        let after = target.as_ref().map(|target| target.id.clone());
        self.session.set(target);
        // 布局事件比"当前会话变了"频繁得多（拖一次分隔条就是一串），
        // 答案没变就别让面板跟着重画。
        if before != after {
            cx.notify();
        }
    }

    // ─── 消息快照 ───────────────────────────────────────────────────────────

    fn set_messages(&mut self, messages: Vec<ChatMessage>, cx: &mut Context<Self>) {
        let count = messages.len();
        self.messages = messages;
        // 换了一批消息，上一批摊开的那几张就跟着作废（id 不会重合，留着也
        // 只是慢慢攒）。新读进来的这批一律收着。
        self.expanded.clear();
        self.scroller.update(cx, |state, cx| state.reset(count, cx));
        cx.notify();
    }

    /// 点了第 `index` 张命令卡片的头：开合翻个面。
    ///
    /// 翻完要重新量这一行 —— 虚拟列表按量出来的高度排版，不量的话摊开的
    /// 输出会被切在旧高度里。
    fn toggle_card(&mut self, id: &SharedString, index: usize, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.clone());
        }
        self.remeasure(index, cx);
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

    // ─── 流式输出 ───────────────────────────────────────────────────────────

    /// 模型又吐出一小段字：接到正在写的那条后面，这一轮还没开口就先开一条。
    fn stream_delta(&mut self, delta: &str, cx: &mut Context<Self>) {
        let Some(streaming) = self.streaming.as_mut() else {
            self.push_message(
                ChatMessage {
                    id: Self::local_id(),
                    role: ChatRole::Assistant,
                    body: MessageBody::Text(delta.into()),
                },
                cx,
            );
            self.streaming = Some(Streaming {
                index: self.messages.len() - 1,
                text: delta.to_owned(),
            });
            return;
        };

        streaming.text.push_str(delta);
        let index = streaming.index;
        let text = SharedString::from(streaming.text.as_str());
        let Some(message) = self.messages.get_mut(index) else {
            return;
        };
        message.body = MessageBody::Text(text);
        self.remeasure(index, cx);
    }

    /// 这一轮说完了：把屏上那条换成规范全文。
    ///
    /// 是**替换**不是追加 —— 增量是暂定的（rig 允许一轮被重试作废），
    /// 而且文字与工具调用的先后在这一步才定下来。这一轮压根没说话（只叫了
    /// 工具）时不会走到这儿；真走到了却没有在写的那条，就当普通消息落一条。
    fn commit_stream(&mut self, body: MessageBody, cx: &mut Context<Self>) {
        let Some(streaming) = self.streaming.take() else {
            return self.push_message(
                ChatMessage {
                    id: Self::local_id(),
                    role: ChatRole::Assistant,
                    body,
                },
                cx,
            );
        };
        let Some(message) = self.messages.get_mut(streaming.index) else {
            return;
        };
        message.body = body;
        self.remeasure(streaming.index, cx);
    }

    /// 第 `index` 行的内容变了，让虚拟列表重新量它。
    ///
    /// 不量的话它还按旧高度排，一边流一边长的那条会被切掉尾巴 ——
    /// 而那恰好是用户正盯着的一行。
    fn remeasure(&mut self, index: usize, cx: &mut Context<Self>) {
        self.scroller.update(cx, |state, cx| {
            state.remeasure_items(index..index + 1, cx);
        });
        cx.notify();
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

        // 发问之前再认一次。两条事件（布局、存活）之间难免有缝，而这一趟
        // 只是走一遍布局树 —— 用户按下发送的那一刻，"助手站在哪台机器上"
        // 不该有第二种答案。
        self.refresh_session(window, cx);

        // 对话历史在这一刻定格；当前会话不定格，它跟着标签页走。
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
        let session = self.session.clone();
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

            // rig 在 tokio 上跑（reqwest 要它的 reactor），这里只当泵。
            //
            // 命令卡片仍是先落库再上屏；模型的话不能这么办 —— 流式输出的
            // 意思就是先上屏，等这一轮说完了（`Text`）再把规范全文落库。
            let run = crate::db::runtime().spawn(assistant::run(RunInput {
                config,
                session,
                history,
                prompt,
                events: events_tx,
                cancel,
            }));

            while let Some(event) = events_rx.next().await {
                let (role, body) = match event {
                    // 一小段字：接到正在写的那条消息尾巴上，不落库。
                    AgentEvent::TextDelta(delta) => {
                        let alive = this.update(cx, |this, cx| this.stream_delta(&delta, cx));
                        if alive.is_err() {
                            return;
                        }
                        continue;
                    }
                    // 这一轮说完了：屏上那条换成规范全文，然后落库。
                    AgentEvent::Text(text) => {
                        let body = MessageBody::Text(text.into());
                        let alive =
                            this.update(cx, |this, cx| this.commit_stream(body.clone(), cx));
                        if alive.is_err() {
                            return;
                        }
                        if let Some(id) = &conversation
                            && let Err(error) =
                                service.append_message(id, ChatRole::Assistant, body).await
                        {
                            log::warn!("助手消息存不下去：{error:#}");
                        }
                        continue;
                    }
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

            // 流里还挂着半条（用户按了停，或者中途出错）就按说到一半的样子
            // 留下来：屏上已经写着它了，重开应用该还在。
            let leftover = this
                .update(cx, |this, _| this.streaming.take())
                .ok()
                .flatten()
                .filter(|streaming| !streaming.text.trim().is_empty());
            if let (Some(id), Some(streaming)) = (&conversation, leftover)
                && let Err(error) = service
                    .append_message(
                        id,
                        ChatRole::Assistant,
                        MessageBody::Text(streaming.text.into()),
                    )
                    .await
            {
                log::warn!("说到一半那条存不下去：{error:#}");
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

    /// 当前对话的标题，截到标签条放得下的长度。
    fn conversation_title(&self) -> SharedString {
        self.active
            .as_ref()
            .and_then(|id| self.conversations.iter().find(|chat| &chat.id == id))
            .map(|chat| {
                if chat.title.chars().count() > TITLE_CHARS {
                    let mut clipped: String = chat.title.chars().take(TITLE_CHARS).collect();
                    clipped.push('…');
                    clipped.into()
                } else {
                    chat.title.clone()
                }
            })
            .unwrap_or_else(|| t!("assistant.new_chat").into())
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
            // 这颗按钮画没画出来，是"还换得了对话吗"唯一问得到的地方 ——
            // 标签条是 Dock 画的，这边只递了一棵元素树上去。非测试档是空操作。
            .debug_selector(|| CONVERSATION_MENU_SELECTOR.to_string())
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
        let expanded = Rc::new(self.expanded.clone());
        let this = cx.entity().downgrade();
        MessageScroller::new(
            "assistant-transcript",
            self.scroller.clone(),
            move |index, _, cx| {
                let Some(message) = messages.get(index) else {
                    return div().into_any_element();
                };
                let disclosure = Disclosure {
                    open: expanded.contains(&message.id),
                    toggle: {
                        let this = this.clone();
                        let id = message.id.clone();
                        Rc::new(move |cx: &mut App| {
                            _ = this.update(cx, |this, cx| this.toggle_card(&id, index, cx));
                        })
                    },
                };
                render_message(message, disclosure, cx)
            },
        )
        .with_bottom_fade(cx.theme().background)
        .size_full()
        .into_any_element()
    }

    /// 当前会话条：助手这一刻能操作的那台机器，就画在输入行上方。
    ///
    /// 它不是装饰：助手只操作当前这条会话，用户切标签页就是换机器 ——
    /// 命令会落在哪儿，得看得见。
    fn render_current_session(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.get();
        let connected = session.is_some();
        let label: SharedString = session
            .map(|target| target.name)
            .unwrap_or_else(|| t!("assistant.no_session").into());

        h_flex()
            .flex_shrink_0()
            .gap_1p5()
            .px_3()
            .pb_1p5()
            .items_center()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(
                Icon::new(AppIcon::Server)
                    .with_size(px(12.))
                    .flex_shrink_0()
                    .text_color(match connected {
                        true => palette::magenta(),
                        false => palette::idle(cx),
                    }),
            )
            .child(div().flex_shrink_0().child(t!("assistant.current_session")))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .when(connected, |this| this.text_color(cx.theme().foreground))
                    .child(label),
            )
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

    /// 输入行：一块把输入框和那颗圆键装在一起的卡片。
    ///
    /// 输入框是 [`Textarea`] 而不是单行输入 —— 问到服务器上的事常常要带一段
    /// 报错或者一段配置进来，单行框把它们压成一条缝，看不见自己发的是什么。
    /// 它跟着内容长（[`COMPOSER_MIN_ROWS`] 起，[`COMPOSER_MAX_ROWS`] 封顶），
    /// 回车发送、Shift+回车换行。
    ///
    /// 发送键落在卡片里的右下角，不摆在输入框旁边：框会长高，旁边那颗按钮
    /// 要么跟着被拉长，要么贴着一块高框的腰飘着。
    ///
    /// 边框、底色和聚焦那圈光环都归卡片（输入框自己那层外观关掉了）：用户
    /// 点进去的是整块卡片，亮起来的就该是整块卡片。
    ///
    /// 运行中输入框还在（可以先打下一句），发送键换成停止键。
    fn render_composer(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.composer.focus_handle(cx).is_focused(window);

        div().flex_shrink_0().px_3().pb_3().child(
            v_flex()
                .w_full()
                .gap_1()
                .p_1p5()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .bg(palette::surface(cx))
                .when(focused, |this| this.focus_ring_style(window, cx))
                .child(Textarea::new(&self.composer).appearance(false).w_full())
                .child(h_flex().justify_end().child(if self.running {
                    Button::new("assistant-stop")
                        .outline()
                        .with_size(SEND_BUTTON_SIZE)
                        .rounded(SEND_BUTTON_SIZE / 2.)
                        .icon(IconName::Pause)
                        .tooltip(t!("assistant.stop"))
                        .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
                } else {
                    Button::new("assistant-send")
                        .primary()
                        .with_size(SEND_BUTTON_SIZE)
                        .rounded(SEND_BUTTON_SIZE / 2.)
                        .icon(IconName::ArrowUp)
                        .tooltip(t!("assistant.send"))
                        .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
                })),
        )
    }
}

impl BasePanel for AssistantPanel {
    fn panel_name(&self) -> &'static str {
        "AssistantPanel"
    }

    /// 关不掉：这块面板没有"另一份"，关掉就只能从活动栏重新建一个，
    /// 而它还攥着这一场运行和那格当前会话。收起来（整条 dock 一起）足够了
    /// —— 中心区标签条右端那颗 `PanelRight`，或活动栏上的 AI 助手键。
    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Panel for AssistantPanel {
    /// 标签条上的标题：图标 + 当前对话，点开换一段。
    ///
    /// 标题就是对话名而不是"智能助手"：这块面板只有一个，叫什么名字不重要，
    /// 而现在在说的是哪一段对话是随时要看的 —— 换对话的入口只有这一个。
    ///
    /// **所以不能实现 [`Panel::tab_name`]。** 它不是"放不下时才用的短名"：
    /// `render_tabs` 一看它有值就整个换掉标题元素，一个纯字符串，菜单连画
    /// 都不画。栽过一次，`the_tab_bar_keeps_the_conversation_menu` 守着它。
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.conversation_title();
        h_flex()
            .items_center()
            .gap_1p5()
            .child(
                Icon::new(AppIcon::Sparkles)
                    .with_size(px(15.))
                    .flex_shrink_0()
                    .text_color(palette::teal()),
            )
            .child(self.render_conversation_menu(title, cx))
    }

    /// 全自动模式亮一块警示牌：命令不送审是要一眼看得见的状态，所以它钉在
    /// 标签条右端，而不是混在对话里。
    fn title_suffix(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<impl IntoElement> {
        assistant_config::global().auto_execute.then(|| {
            div()
                .flex_shrink_0()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .text_xs()
                .bg(palette::amber().opacity(0.15))
                .text_color(palette::amber())
                .child(t!("assistant.auto_badge"))
        })
    }

    /// 标签条右端那两颗：新开一段、删掉这一段。样式归 Dock（它统一按
    /// `xsmall().ghost()` 画），这里只说是哪两件事。
    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        Some(vec![
            Button::new("assistant-new-chat")
                .icon(IconName::Plus)
                .tooltip(t!("assistant.new_chat"))
                .disabled(self.running)
                .on_click(cx.listener(|this, _, _, cx| this.new_conversation(cx))),
            Button::new("assistant-delete-chat")
                .icon(Icon::new(AppIcon::Trash))
                .tooltip(t!("assistant.delete_chat"))
                .disabled(self.running || self.active.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.delete_conversation(cx))),
        ])
    }

    /// 自己贴着边排（输入行有自己的内边距），不要 Dock 再给一层。
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for AssistantPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("assistant-panel")
            .size_full()
            .bg(cx.theme().background)
            .child(div().flex_1().min_h_0().child(self.render_transcript(cx)))
            // 运行指示：泵还开着、没在等审核、也还没开口说话时，说明模型在想
            // 或者命令在跑。字一旦流出来，那行字本身就是进度，不必再转圈。
            .when(
                self.running && self.pending.is_none() && self.streaming.is_none(),
                |this| {
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
                },
            )
            .map(|this| match self.pending.as_ref() {
                Some(pending) => this.child(self.render_approval(pending, cx)),
                None => this,
            })
            .child(self.render_current_session(cx))
            .child(self.render_composer(window, cx))
    }
}

/// 一张命令卡片的开合：现在是摊开的吗，点一下该翻给谁。
///
/// 行渲染器一次只交出一条消息，挑哪种画法的是 [`render_message`] 自己 ——
/// 所以文字消息也会收到一份，只是用不上。
struct Disclosure {
    open: bool,
    /// `Rc` 是给点击回调的：那个回调要 `'static`，而这份开关是每帧现做的。
    toggle: Rc<dyn Fn(&mut App)>,
}

/// 一条消息画成一行。独立函数：MessageScroller 的行渲染器拿不到 `Context<Self>`。
///
/// 两边的话都走同一个 [`Message`]，差的只是对齐和气泡档位：用户的话靠右、
/// 一块 Muted 的实底气泡（组件库给它 80% 的宽上限，短句子就是短的一块）；
/// 助手的话靠左、Ghost 档 —— 没有面、没有内边距、铺满整宽，长 Markdown 在
/// 380px 的面板里不该再被一圈内边距挤掉两个字。
///
/// 脚注里那颗复制键两边都有。Ghost 那档 [`MessageFooter`] 会自己去掉横向
/// 内边距（`MessageContent::bubble` 把"这是个没有面的气泡"往外传），所以
/// 按钮和正文左边缘对得齐，实底气泡那边则跟着气泡的内边距缩进。
fn render_message(message: &ChatMessage, disclosure: Disclosure, cx: &App) -> AnyElement {
    match &message.body {
        MessageBody::Text(text) => {
            let from_user = message.role == ChatRole::User;
            Message::new()
                .alignment(match from_user {
                    true => MessageAlignment::End,
                    false => MessageAlignment::Start,
                })
                .content(
                    MessageContent::new().bubble(match from_user {
                        true => Bubble::new()
                            .with_variant(BubbleVariant::Muted)
                            .child(text.clone()),
                        // Markdown 自己排版，气泡只负责不挡路。
                        false => Bubble::new().with_variant(BubbleVariant::Ghost).child(
                            div()
                                .w_full()
                                .min_w_0()
                                .child(TextView::markdown(message.id.clone(), text.clone())),
                        ),
                    }),
                )
                .footer(MessageFooter::new().child(render_copy_button(&message.id, text)))
                .into_any_element()
        }
        MessageBody::Command {
            command,
            session,
            output,
            status,
        } => render_command_card(message, command, session, output, *status, disclosure, cx),
    }
}

/// 把这条消息的原文复制走。
///
/// 复制的是 Markdown 源码，不是画出来的那个样子：这段话多半要粘进工单、
/// 提交信息或者另一个聊天窗口，那些地方认的正是源码。
fn render_copy_button(id: &SharedString, text: &SharedString) -> Button {
    let text = text.clone();
    Button::new(SharedString::from(format!("assistant-copy-{id}")))
        .ghost()
        .xsmall()
        .icon(IconName::Copy)
        .tooltip(t!("assistant.copy"))
        .debug_selector(|| COPY_SELECTOR.to_string())
        .on_click(move |_, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
            window.push_notification(t!("assistant.copied"), cx);
        })
}

/// 命令执行卡片：状态行、命令、输出（有的话）。
///
/// **默认收着**：只留状态行和一行截断的命令 —— 一次 `df -hT` 就能吐几十行，
/// 几张摊开的卡片会把对话本身顶出屏幕。命令那行不跟着收：跑了什么是这张卡片
/// 的身份，收起来就只剩"在 web-01 上成功了"这种废话。
///
/// 点卡片头（状态行和命令那行都算）展开：命令换成完整的，输出跟着上屏。
fn render_command_card(
    message: &ChatMessage,
    command: &SharedString,
    session: &SharedString,
    output: &SharedString,
    status: CommandStatus,
    disclosure: Disclosure,
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

    let Disclosure { open, toggle } = disclosure;

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
            // 开合区：状态行加命令那行一起当把手。命令常常长得要折行，
            // 让它也能点比逼用户去瞄那一行状态准得多。
            v_flex()
                .id(SharedString::from(format!(
                    "assistant-command-head-{}",
                    message.id
                )))
                .w_full()
                .min_w_0()
                .gap_1p5()
                .cursor_pointer()
                .debug_selector(|| CARD_HEAD_SELECTOR.to_string())
                .on_click(move |_, _, cx| toggle(cx))
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
                        .child(div().flex_shrink_0().text_color(tone).child(label))
                        .child(
                            Icon::new(match open {
                                true => IconName::ChevronDown,
                                false => IconName::ChevronRight,
                            })
                            .with_size(px(12.))
                            .flex_shrink_0()
                            .text_color(cx.theme().muted_foreground),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_sm()
                        // 收着的时候只给一行：命令本身可以是一长串用 `;`
                        // 串起来的东西，摊开来就跟输出一样占地方。
                        .when(!open, |this| this.truncate())
                        .child(command.clone()),
                ),
        )
        .when(open && !output.is_empty(), |this| {
            // 摊开的这一份还要再紧一档：工具层喂给模型的已经截到 8000 字，
            // 画出来还是能顶掉整屏。
            let shown: SharedString = if output.chars().count() > OUTPUT_DISPLAY_CHARS {
                let mut clipped: String = output.chars().take(OUTPUT_DISPLAY_CHARS).collect();
                clipped.push('…');
                clipped.into()
            } else {
                output.clone()
            };
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
                    .debug_selector(|| CARD_OUTPUT_SELECTOR.to_string())
                    .child(shown),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt as _;
    use gpui_kit::component::dock::{
        DockLayout, DockPlacement, DockSkin, PanelStyle, panel_handle,
    };

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

    /// 三种消息画法齐上阵地画一整帧：气泡、Markdown、三种状态的命令卡片，
    /// 外加标签条上那三样（对话菜单、警示牌、两颗按钮）。深 builder 链上的
    /// 错只有真的画一遍才掉得出来。
    ///
    /// 全自动开着画：[`Panel::title_suffix`] 那块警示牌只有这一档才画。
    #[gpui_kit::test]
    fn the_panel_draws_every_message_shape(cx: &mut gpui_kit::TestAppContext) {
        let _exclusive = assistant_config::exclusive_for_test();
        assistant_config::set_global(assistant_config::AssistantConfig {
            auto_execute: true,
            ..Default::default()
        });

        let (harness, cx) = dock_harness(cx, sample_messages());
        // 第二帧走的不是同一条路：虚拟列表的量测只有这一帧才读得到。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        harness.read_with(cx, |harness, cx| {
            let panel = harness.assistant.read(cx);
            assert_eq!(panel.messages.len(), 4);
            assert!(!panel.running);
        });
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
                command: "systemctl restart nginx".into(),
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
                MessageBody::Text(text) => text.as_ref(),
                MessageBody::Command { .. } => panic!("这条该是文字消息"),
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
            panel.commit_stream(MessageBody::Text("根分区还有 **28G** 可用。".into()), cx);
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
                body: MessageBody::Text(TEXT.into()),
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
        assistant_config::set_global(assistant_config::AssistantConfig {
            api_key: "test-key".into(),
            model: "test-model".into(),
            ..Default::default()
        });

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
}
