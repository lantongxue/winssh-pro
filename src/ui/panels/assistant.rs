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
//! **图片跟着提问一起发。** 模型开了看图
//! （[`AssistantConfig::vision`](crate::assistant::AssistantConfig::vision)）时，
//! 输入行左下角多出一颗粘贴图片的键；在输入框里 Ctrl+V 一张图也一样。图从哪儿
//! 来归用户自己（系统截图、别的截图工具、浏览器里复制的图、资源管理器里复制的
//! 图片文件），这里只认剪贴板。贴上的图先排在输入框上方
//! （[`AssistantPanel::drafts`]），后台规整成模型收的尺寸与格式
//! （[`ChatImage::prepare`]），按发送时随这一条提问一起走、一起落库。没开看图
//! 时那颗键不画，Ctrl+V 一张图只换来一句"去设置里打开"——往纯文本模型发图
//! 换来的只是一条 400。
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
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    ThemeStyled as _, WindowExt as _,
    attachment::{
        Attachment, AttachmentActions, AttachmentGroup, AttachmentMedia, AttachmentStatus,
    },
    bubble::{Bubble, BubbleVariant},
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, DockArea, DockEvent, Panel, PanelEvent},
    h_flex,
    hover_card::HoverCard,
    input::{InputEvent, Textarea, TextareaState},
    menu::{DropdownMenu as _, PopupMenuItem},
    message::{Message, MessageAlignment, MessageContent, MessageFooter},
    message_scroller::{MessageScroller, MessageScrollerState},
    spinner::Spinner,
    text::TextView,
    v_flex,
};
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Axis, ClipboardEntry, ClipboardItem, Context,
    ElementId, Entity, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, WeakEntity, Window, div, img, prelude::FluentBuilder as _, px,
};

use crate::assistant::{
    self, AgentEvent, ChatImage, ChatMessage, ChatRole, CommandStatus, Conversation,
    CurrentSession, MessageBody, RunInput, Turn, config as assistant_config,
    image::looks_like_image,
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

/// 停止键那颗图标按多大的按钮来算 —— 只管图标，圆本身仍是
/// [`SEND_BUTTON_SIZE`]。
///
/// 箭头是一笔细线，两根几乎顶满画布的实心竖条不是：同样按圆的 0.75 倍画
/// 出来，Pause 比 ArrowUp 重一大截，像是把整个圆填满了。[`Button`] 不收
/// 单独的图标尺寸（[`Button::icon`] 收到的那颗会被按钮尺寸覆写），所以这里
/// 反过来给它一个小一号的按钮尺寸，让那 0.75 倍落在合适的地方。
const STOP_ICON_BOX: Pixels = px(21.);

/// 一条提问最多贴几张图。每张图每一轮都要随整段上下文重发一遍，五张已经是
/// 七八千个 token；真要对照更多截图，分几次问比一次塞进去答得好。
const MAX_DRAFTS: usize = 5;

/// 输入行上那排缩略图的边长。比消息流里的小一号：它只是"待发的有哪几张"，
/// 看清内容靠悬停预览。
const DRAFT_THUMB: Pixels = px(64.);

/// 消息流里缩略图的边长。定死是给虚拟列表的：行高要能量得准，图是后台解码
/// 的，解完之前和之后必须一样高。
const MESSAGE_THUMB: Pixels = px(96.);

/// 悬停预览里那张图最长边画多大。
const PREVIEW_EDGE: f32 = 360.;

/// 输入行上缩略图那颗移除键的调试名。每张一个同名的，测试只摆一张（同复制键）。
const DRAFT_REMOVE_SELECTOR: &str = "assistant-draft-remove";

/// 一条等着用户答复的命令。`respond` 送回 `true`（批准）或 `false`（拒绝）；
/// 直接丢掉它（停止运行时）等于让运行收口。
struct PendingApproval {
    session: SharedString,
    command: SharedString,
    respond: Option<oneshot::Sender<bool>>,
}

/// 输入行上一张等着随下一条提问发出去的图。
struct Draft {
    /// 界面本地的编号：移除键与悬停预览的元素 id 靠它，规整完回来也按它认。
    id: SharedString,
    /// `None`：还在后台规整（解码、缩放、转码），画成一块转圈的占位。
    image: Option<ChatImage>,
}

/// 一张要贴上来的图从哪儿来。
enum Incoming {
    /// 已经在内存里的编码字节：剪贴板里的图、截到的图。
    Bytes(Vec<u8>),
    /// 一个图片文件：剪贴板里复制的是文件（资源管理器里 Ctrl+C 一张截图）。
    File(PathBuf),
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
    /// 输入行上等着随下一条提问发出去的图，按贴进来的先后。换对话不清空 ——
    /// 和输入框里打了一半的字一样，是用户还没发出去的东西。
    drafts: Vec<Draft>,
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
            drafts: Vec::new(),
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
                    body: MessageBody::text(delta.to_owned()),
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
        message.body = MessageBody::text(text);
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

    // ─── 贴图 ───────────────────────────────────────────────────────────────

    /// 往输入行上贴几张图。规整在后台做：先占一块转圈的位子，规整完换成
    /// 缩略图，规整不了的撤掉并说一声。
    ///
    /// 模型没开看图就一张都不收 —— 输入行上那两颗键这时本来就不画，走到这里的
    /// 只会是 Ctrl+V。超出 [`MAX_DRAFTS`] 的那几张不收，也说一声。
    fn attach(&mut self, incoming: Vec<Incoming>, window: &mut Window, cx: &mut Context<Self>) {
        if incoming.is_empty() {
            return;
        }
        if !assistant_config::global().vision {
            window.push_notification(t!("assistant.vision_off"), cx);
            return;
        }
        let room = MAX_DRAFTS.saturating_sub(self.drafts.len());
        if incoming.len() > room {
            window.push_notification(t!("assistant.too_many_images", count => MAX_DRAFTS), cx);
        }

        for source in incoming.into_iter().take(room) {
            let id = Self::local_id();
            self.drafts.push(Draft {
                id: id.clone(),
                image: None,
            });
            // 一张 4K 截图解码、缩放、再编码要上百毫秒，不能在 gpui 线程上做。
            let prepare = cx.background_spawn(async move {
                match source {
                    Incoming::Bytes(bytes) => ChatImage::prepare(&bytes),
                    Incoming::File(path) => ChatImage::open(&path),
                }
            });
            cx.spawn_in(window, async move |this, cx| {
                let result = prepare.await;
                _ = this.update_in(cx, |this, window, cx| {
                    this.prepared(&id, result, window, cx);
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// 后台规整完一张。这期间用户可能已经把它移除了，那就什么都不做。
    fn prepared(
        &mut self,
        id: &SharedString,
        result: anyhow::Result<ChatImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.drafts.iter().position(|draft| &draft.id == id) else {
            return;
        };
        match result {
            Ok(image) => self.drafts[index].image = Some(image),
            Err(error) => {
                self.drafts.remove(index);
                window.push_notification(
                    t!("assistant.image_failed", error => format!("{error:#}")),
                    cx,
                );
            }
        }
        cx.notify();
    }

    fn remove_draft(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        self.drafts.retain(|draft| &draft.id != id);
        cx.notify();
    }

    /// 输入框里按了粘贴（Ctrl+V / 右键菜单）：剪贴板里是图就接过来，
    /// 返回 `true` 让输入框别再插字。
    ///
    /// **剪贴板里有字就按字贴。** 从 Excel、Word、网页复制出来的东西常常字和
    /// 一张渲染图一起带着，这时候用户要的是字；截图工具放进剪贴板的只有图。
    /// 真想要那张图，输入行左下角的粘贴键只认图。
    fn paste_into_composer(
        &mut self,
        item: &ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let has_text = item.entries().iter().any(
            |entry| matches!(entry, ClipboardEntry::String(string) if !string.text.is_empty()),
        );
        if has_text {
            return false;
        }
        let images = clipboard_images(item);
        if images.is_empty() {
            return false;
        }
        self.attach(images, window, cx);
        true
    }

    /// 输入行上那颗粘贴键：剪贴板里有图（或图片文件）就贴上来，字不管。
    fn paste_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let images = cx
            .read_from_clipboard()
            .map(|item| clipboard_images(&item))
            .unwrap_or_default();
        if images.is_empty() {
            window.push_notification(t!("assistant.no_clipboard_image"), cx);
            return;
        }
        self.attach(images, window, cx);
    }

    // ─── 发送与运行 ─────────────────────────────────────────────────────────

    /// 发出去：输入框里的字连同输入行上的图。只贴了图、一个字没写也能发。
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let prompt = self.composer.read(cx).value().trim().to_string();
        if prompt.is_empty() && self.drafts.is_empty() {
            return;
        }
        let config = assistant_config::global();
        if !config.ready() {
            window.push_notification(t!("assistant.not_configured"), cx);
            return;
        }
        if !self.drafts.is_empty() {
            // 图贴上之后又去设置里关了看图：不替用户做主把图扔掉 —— 是移除这几张图，
            // 还是回设置里打开，由用户自己挑。
            if !config.vision {
                window.push_notification(t!("assistant.vision_off"), cx);
                return;
            }
            if self.drafts.iter().any(|draft| draft.image.is_none()) {
                window.push_notification(t!("assistant.image_preparing"), cx);
                return;
            }
        }
        let images: Vec<ChatImage> = self
            .drafts
            .drain(..)
            .filter_map(|draft| draft.image)
            .collect();
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));

        // 发问之前再认一次。两条事件（布局、存活）之间难免有缝，而这一趟
        // 只是走一遍布局树 —— 用户按下发送的那一刻，"助手站在哪台机器上"
        // 不该有第二种答案。
        self.refresh_session(window, cx);

        // 对话历史在这一刻定格；当前会话不定格，它跟着标签页走。
        let turns: Vec<Turn> = self
            .messages
            .iter()
            .filter_map(|message| match &message.body {
                MessageBody::Text { text, images } => Some(Turn {
                    role: message.role,
                    text: text.to_string(),
                    images: images.clone(),
                }),
                MessageBody::Command { .. } => None,
            })
            .collect();
        let start = turns.len().saturating_sub(HISTORY_LIMIT);
        let history = turns[start..].to_vec();

        let body = MessageBody::Text {
            text: prompt.clone().into(),
            images: images.clone(),
        };
        self.push_message(
            ChatMessage {
                id: Self::local_id(),
                role: ChatRole::User,
                body: body.clone(),
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
                && let Err(error) = service.append_message(id, ChatRole::User, body).await
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
                images,
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
                        let body = MessageBody::text(text);
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
                    .append_message(id, ChatRole::Assistant, MessageBody::text(streaming.text))
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
                            body: MessageBody::text(t!("assistant.run_failed", error => reason)),
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
    ///
    /// 贴上的图排在输入框上方（[`Self::render_drafts`]）；粘贴图片那颗键在
    /// 左下角，和发送键同一行 —— 只在模型开了看图时才画。
    fn render_composer(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.composer.focus_handle(cx).is_focused(window);
        let vision = assistant_config::global().vision;
        let this = cx.entity().downgrade();

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
                .when(!self.drafts.is_empty(), |this| {
                    this.child(self.render_drafts(cx))
                })
                .child(
                    Textarea::new(&self.composer)
                        .appearance(false)
                        .w_full()
                        .on_paste(move |item, window, cx| {
                            this.update(cx, |this, cx| this.paste_into_composer(item, window, cx))
                                .unwrap_or(false)
                        }),
                )
                .child(
                    h_flex()
                        .when(vision, |this| {
                            this.child(
                                Button::new("assistant-paste-image")
                                    .ghost()
                                    .small()
                                    .icon(AppIcon::ClipboardPaste)
                                    .tooltip(t!("assistant.paste_image"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.paste_image(window, cx)
                                    })),
                            )
                        })
                        .child(self.render_send_button(cx).ml_auto()),
                ),
        )
    }

    /// 发送键，运行中换成停止键。
    fn render_send_button(&self, cx: &mut Context<Self>) -> Button {
        if self.running {
            Button::new("assistant-stop")
                .outline()
                // 图标按这一档算（见 [`STOP_ICON_BOX`]），圆按下一行。
                .with_size(STOP_ICON_BOX)
                .size(SEND_BUTTON_SIZE)
                .rounded(SEND_BUTTON_SIZE / 2.)
                .icon(AppIcon::Squircle)
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
        }
    }

    /// 输入框上方那一排贴上的图（规整完之前是一块转圈的占位）。每张图右上角
    /// 一颗移除键，停在缩略图上看整张。
    fn render_drafts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        AttachmentGroup::new("assistant-drafts")
            .gap_2()
            .children(self.drafts.iter().map(|draft| {
                let remove = {
                    let id = draft.id.clone();
                    Button::new(SharedString::from(format!(
                        "assistant-draft-remove-{}",
                        draft.id
                    )))
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("assistant.remove_image"))
                    .debug_selector(|| DRAFT_REMOVE_SELECTOR.to_string())
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_draft(&id, cx)))
                };
                let card = Attachment::new()
                    .axis(Axis::Vertical)
                    .small()
                    .w(DRAFT_THUMB)
                    .status(match draft.image {
                        Some(_) => AttachmentStatus::Complete,
                        None => AttachmentStatus::Processing,
                    })
                    .media(match &draft.image {
                        Some(image) => AttachmentMedia::new().src(image.image()),
                        None => AttachmentMedia::new().overlay(Spinner::new().small()),
                    })
                    .actions(AttachmentActions::new().child(remove));
                match &draft.image {
                    Some(image) => preview_on_hover(
                        SharedString::from(format!("assistant-draft-preview-{}", draft.id)),
                        image,
                        card,
                    ),
                    None => card.into_any_element(),
                }
            }))
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
///
/// 贴着图的提问，图排在气泡上方（[`render_message_images`]）；只贴了图、没写
/// 字的那条不画气泡，也就没有复制键 —— 没有字可复制。
fn render_message(message: &ChatMessage, disclosure: Disclosure, cx: &App) -> AnyElement {
    match &message.body {
        MessageBody::Text { text, images } => {
            let from_user = message.role == ChatRole::User;
            let has_text = !text.is_empty();
            Message::new()
                .alignment(match from_user {
                    true => MessageAlignment::End,
                    false => MessageAlignment::Start,
                })
                .content(
                    MessageContent::new()
                        .when(!images.is_empty(), |this| {
                            this.child(render_message_images(&message.id, images))
                        })
                        .when(has_text, |this| {
                            this.bubble(match from_user {
                                true => Bubble::new()
                                    .with_variant(BubbleVariant::Muted)
                                    .child(text.clone()),
                                // Markdown 自己排版，气泡只负责不挡路。
                                false => Bubble::new().with_variant(BubbleVariant::Ghost).child(
                                    div().w_full().min_w_0().child(TextView::markdown(
                                        message.id.clone(),
                                        text.clone(),
                                    )),
                                ),
                            })
                        }),
                )
                .when(has_text, |this| {
                    this.footer(MessageFooter::new().child(render_copy_button(&message.id, text)))
                })
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

/// 一条提问贴着的那几张图：定死边长的方形缩略图，靠右排、放不下就折行，
/// 停上去看整张。
///
/// 边长定死（[`MESSAGE_THUMB`]）是给虚拟列表的：图在后台解码，解完之前和
/// 之后这一行必须一样高，不然量出来的行高会跟着跳。
fn render_message_images(message_id: &SharedString, images: &[ChatImage]) -> impl IntoElement {
    h_flex()
        .max_w_full()
        .flex_wrap()
        .justify_end()
        .gap_1p5()
        .children(images.iter().enumerate().map(|(index, image)| {
            preview_on_hover(
                SharedString::from(format!("assistant-image-{message_id}-{index}")),
                image,
                Attachment::new()
                    .axis(Axis::Vertical)
                    .w(MESSAGE_THUMB)
                    .media(AttachmentMedia::new().src(image.image())),
            )
        }))
}

/// 给一块缩略图套上悬停预览：整张图按比例缩进 [`PREVIEW_EDGE`] 见方，下面
/// 一行写着尺寸与格式 —— 发出去的就是这一张，用户该看得见模型会看到什么。
fn preview_on_hover(
    id: impl Into<ElementId>,
    image: &ChatImage,
    trigger: impl IntoElement + 'static,
) -> AnyElement {
    let scale = (PREVIEW_EDGE / image.width().max(image.height()).max(1) as f32).min(1.);
    let (width, height) = (
        (image.width() as f32 * scale).max(1.),
        (image.height() as f32 * scale).max(1.),
    );
    let source = image.image();
    let caption: SharedString = format!(
        "{} × {} · {}",
        image.width(),
        image.height(),
        match image.is_png() {
            true => "PNG",
            false => "JPEG",
        }
    )
    .into();

    HoverCard::new(id)
        .anchor(Anchor::BottomCenter)
        .trigger(trigger)
        .content(move |_, _, cx| {
            v_flex()
                .gap_1p5()
                .child(img(source.clone()).w(px(width)).h(px(height)).rounded_md())
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(caption.clone()),
                )
        })
        .into_any_element()
}

/// 剪贴板里能当图贴上来的东西：嵌在剪贴板里的图本身（截图工具、浏览器
/// "复制图片"），以及复制下来的图片文件（资源管理器里 Ctrl+C）。字不算。
fn clipboard_images(item: &ClipboardItem) -> Vec<Incoming> {
    item.entries()
        .iter()
        .flat_map(|entry| match entry {
            ClipboardEntry::Image(image) => vec![Incoming::Bytes(image.bytes().to_vec())],
            ClipboardEntry::ExternalPaths(paths) => paths
                .paths()
                .iter()
                .filter(|path| looks_like_image(path))
                .map(|path| Incoming::File(path.clone()))
                .collect(),
            ClipboardEntry::String(_) => Vec::new(),
        })
        .collect()
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
#[path = "../../tests/ui/panels/assistant.rs"]
mod tests;
