//! rig 那一侧：把一次用户提问跑成一场多轮的 Agent 运行。
//!
//! 结构故意压得很平：一个工具（[`RunCommand`]，在当前这条会话上执行命令）、
//! 一个钩子（[`Gate`]，人工审核 + 停止 + 把进展报给界面）、一个入口
//! （[`run`]）。多轮循环、工具调度、无效调用恢复都归 rig 的
//! [`rig_agent::AgentRunner`]，这里不重写它。
//!
//! **助手只操作当前打开的那条会话。** 它够得着的机器只有一台 ——
//! 中心区里在前面的那个标签页（[`CurrentSession`]）。这同样不是一句提示词
//! 的事：工具压根没有"选一台机器"的参数，执行前现读这一格。用户切标签页
//! 就是换机器，[`Gate::on_completion_call`] 在下一次打模型之前把新的那条
//! 写进系统提示词，所以模型永远知道自己站在哪儿。
//!
//! 界面与运行之间只有一条事件通道（[`AgentEvent`]）：模型说的话、命令的
//! 执行结果、待审核的命令都从这里出去。审核是一问一答 —— 事件里带一个
//! `oneshot`，界面按下"批准/拒绝"把答案送回来，钩子在 [`Gate::on_tool_call`]
//! 里等着它。全自动模式跳过这一问。
//!
//! **流式输出走的是 rig 的 `stream()` 那一侧。** 同一套 builder、同一个引擎、
//! 同一批钩子，只是模型那头改走 SSE，并且多给两类观察事件；换来的是
//! [`Gate::on_text_delta`]（模型每吐一小段字就报一次，[`AgentEvent::TextDelta`]）
//! 与 [`Gate::on_stream_response_finish`]（这一轮说完了，规范全文，
//! [`AgentEvent::Text`]）。非流式那边的 `on_completion_response` 在这条路上
//! 根本不会来，别在那儿挂东西。增量是暂定的（一轮可以被重试作废），所以
//! 落库的永远是收尾那一份，界面拿它整条替换。
//!
//! **助手不碰本机文件。** 同样是结构上做不到：它没有任何本地文件工具可调，
//! 唯一的工具走的是 SSH 连接上另开的 exec 通道（[`crate::ssh::exec_script`]），
//! 和终端、SFTP 互不干扰。
//!
//! reqwest 要 tokio 的 reactor，所以 [`run`] 必须经由 [`crate::db::run`]
//! 派到应用级 tokio runtime 上，界面侧 `await` 到的只是个信封。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui_kit::SharedString;
use rig_agent::agent::{
    CompletionCallAction, CompletionCallEvent, ObservationAction, RequestPatch,
    StreamResponseFinish, TextDelta, ToolCall as ToolCallEvent, ToolCallAction,
};
use rig_agent::client::AgentModelExt as _;
use rig_agent::completion::Message;
use rig_agent::tool::{Tool, ToolContext};
use rig_agent::{AgentHook, HookContext};
use rig_core::client::completion::CompletionClient as _;
use rig_core::completion::{AssistantContent, CompletionModel};
use rig_core::providers::{anthropic, openai};
use russh::client::Handle;
use serde::Deserialize;

use crate::assistant::{AssistantConfig, ChatRole, ProviderProtocol};
use crate::ssh::{SshHandler, exec_script};
use rust_i18n::t;

/// 一场运行最多打给模型多少次。够跑完一串"查看 → 判断 → 处理"的循环，
/// 又不至于让一个跑偏的模型无限烧下去。
const MAX_TURNS: usize = 16;

/// 一条命令默认等多久、最多等多久。
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 300;

/// 送回给模型和界面的输出上限（字符数）。远端刷屏不能把上下文窗口吃光。
const MAX_TOOL_OUTPUT: usize = 8_000;

/// 系统提示词模板。`{session}` 替换成当前这条会话。
///
/// 是常量不是 i18n 条目：这是说给模型听的，不该跟着界面语言变 ——
/// 里面已经写明"用用户提问的语言回答"。
const PREAMBLE: &str = "\
You are the operations assistant inside WinSSH Pro, an SSH client.

You work on exactly ONE machine: the session the user currently has open in \
the front tab. The `run_command` tool always runs there. You cannot choose \
another host and you cannot see the user's other tabs.

Current session (name | login | platform):
{session}

Rules:
- The user may switch tabs at any moment, which switches the machine under \
you. The block above is rewritten before every reply, so it always names the \
machine you are on right now. When it changes mid-conversation, everything you \
collected earlier came from the previous machine — say so instead of mixing \
the two, and re-check anything that still matters.
- Commands are executed by a non-interactive POSIX /bin/sh without a TTY. Never \
run interactive or long-lived programs (top, vim, tail -f, watch); use one-shot \
alternatives (ps, df, tail -n 100).
- You have NO access to the user's local machine (the SSH client itself) and no \
local file tools. If asked to read or write local files, say plainly that you \
cannot.
- By default every command you run is shown to the user for approval first; a \
declined command was NOT executed. Explain risky operations (deleting data, \
restarting services, changing firewalls) before running them.
- When no session is open, help with advice and say the user has to open and \
connect one before you can run anything.
- Answer in the language the user writes in.";

/// 眼下没有会话在前面时替进 `{session}` 的那一行。
const NO_SESSION: &str = "(no session is open right now)";

/// 用户在审核里拒绝之后回给模型的话。
const DENIED_FEEDBACK: &str =
    "The user declined to run this command. It was NOT executed. Ask or adjust before retrying.";

/// 没有会话可操作时回给模型的话。
const NO_SESSION_FEEDBACK: &str = "No session is open in the app right now, so the command was \
NOT executed. Tell the user to open and connect one first.";

/// 审核卡片还挂着的时候用户换了台机器，那张卡片上的批准就不作数了。
const SWITCHED_FEEDBACK: &str = "The user switched to another session while this command was \
waiting for approval, so it was NOT executed. Check where you are now before trying again.";

/// 用户按下"停止"之后收口用的原因。
const STOPPED: &str = "The user stopped this run.";

/// 一条已连接的会话在助手眼里的样子：够模型认出这是哪台机器，够工具拿到通道。
///
/// `handle` 是那条 SSH 连接本体的共享句柄 —— SSH 是多路复用的，命令走的是
/// 连接上**另开**的一条 exec 通道，用户正在敲的终端不受影响。
#[derive(Clone)]
pub struct SessionTarget {
    /// 会话 id（主机 id）。不给模型看，只用来认"这还是不是刚才那一台"。
    pub id: SharedString,
    /// 显示名，审核卡片与执行卡片上写的是它。
    pub name: SharedString,
    /// `user@hostname`，帮模型分清同名主机。
    pub login: SharedString,
    /// 操作系统标识，模型选命令的写法要看它。
    pub platform: SharedString,
    pub handle: Arc<Handle<SshHandler>>,
}

/// 助手能操作的那台机器：一格随标签页换内容的共享槽。
///
/// 界面在标签页切换、会话连上或断开时换掉里面这条（[`CurrentSession::set`]），
/// 运行侧则在每次打模型、每次执行前重新读一遍。所以"只操作当前打开的会话"
/// 是**读的那一刻**成立的事实，而不是按下发送时的一张快照 —— 一场运行跨越
/// 几十秒，这中间用户完全可能换一台机器看。
///
/// 空着（`None`）是正常状态：眼下没有会话在前面，或者前面那条是本机 Shell、
/// 是已经断掉的会话。
#[derive(Clone, Default)]
pub struct CurrentSession(Arc<Mutex<Option<SessionTarget>>>);

impl CurrentSession {
    /// 换掉当前会话。`None` = 眼下没有可操作的机器。
    pub fn set(&self, target: Option<SessionTarget>) {
        *self.lock() = target;
    }

    /// 读一眼当前会话。
    pub fn get(&self) -> Option<SessionTarget> {
        self.lock().clone()
    }

    /// 中毒的锁照常用：这格里就一个 `Option`，没有"改到一半"的中间态。
    fn lock(&self) -> MutexGuard<'_, Option<SessionTarget>> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// 运行过程中发给界面的一条事件。
pub enum AgentEvent {
    /// 模型刚吐出来的一小段字。一轮里来很多条，界面接在上一段后面。
    ///
    /// 这些是**暂时的**：模型这一轮还没说完，rig 也还没把它规范化
    /// （文字与思考、工具调用的先后会重排）。所以只上屏，不落库。
    TextDelta(String),
    /// 模型这一轮说完了，这是规范化之后的全文。
    ///
    /// 它**替换**刚才那一串增量拼出来的，不是接在后面 —— 一来那串增量
    /// 可能被重试作废过（rig 把流里的增量算作暂定的），二来落库的该是
    /// 这一份。一轮里只说工具不说话时不来这一条。
    Text(String),
    /// 一条命令跑完了（含失败）。被拒绝的命令不走这里 ——
    /// 界面在拒绝那一下自己记卡片。
    Command {
        session: SharedString,
        command: String,
        /// 给人看的输出（stdout 与 stderr 合并，截断过）。
        output: String,
        ok: bool,
    },
    /// 一条命令在等用户批准。
    Approval(ApprovalRequest),
}

/// 待审核的一条命令。界面按"批准/拒绝"把答案从 `respond` 送回来；
/// 直接丢掉它等于拒绝（钩子把发送端关闭当 Stop 处理）。
pub struct ApprovalRequest {
    pub session: SharedString,
    pub command: String,
    pub respond: oneshot::Sender<bool>,
}

/// 一次运行要的全部东西。
pub struct RunInput {
    pub config: AssistantConfig,
    /// 当前这条会话。**不是快照**：界面握着同一格，切标签页就地生效。
    pub session: CurrentSession,
    /// 这段对话此前的问答（只有文字，命令卡片不进历史 ——
    /// 模型要的话自己再查，比一段过期的输出可靠）。
    pub history: Vec<(ChatRole, String)>,
    pub prompt: String,
    pub events: mpsc::UnboundedSender<AgentEvent>,
    /// 界面的"停止"按钮。置位后钩子在下一个模型调用/工具调用边界收口。
    pub cancel: Arc<AtomicBool>,
}

/// 跑一场。必须在 tokio runtime 上（经由 [`crate::db::run`]）。
///
/// 模型的话与命令结果都从 `events` 出去，这里返回的 `Ok(())` 只说明
/// 运行正常收尾；被"停止"按钮收口的运行也算正常收尾，不当错误报。
pub async fn run(input: RunInput) -> Result<()> {
    let config = input.config.clone();
    match config.protocol {
        ProviderProtocol::OpenAi => {
            // Chat Completions 那一档，不是 Responses API：DeepSeek、Zhipu、
            // 千问的兼容端点都只讲前者。
            let mut builder = openai::CompletionsClient::builder().api_key(config.api_key.as_str());
            if !config.base_url.trim().is_empty() {
                builder = builder.base_url(config.base_url.trim());
            }
            let client = builder
                .build()
                .with_context(|| t!("assistant.client_failed").to_string())?;
            drive(client.completion_model(config.model.trim()), input).await
        }
        ProviderProtocol::Anthropic => {
            let mut builder = anthropic::Client::builder().api_key(config.api_key.as_str());
            if !config.base_url.trim().is_empty() {
                builder = builder.base_url(config.base_url.trim());
            }
            let client = builder
                .build()
                .with_context(|| t!("assistant.client_failed").to_string())?;
            drive(client.completion_model(config.model.trim()), input).await
        }
    }
}

/// 协议无关的那一半：装配 Agent 并跑完。
async fn drive<M>(model: M, input: RunInput) -> Result<()>
where
    M: CompletionModel + 'static,
{
    let RunInput {
        config,
        session,
        history,
        prompt,
        events,
        cancel,
    } = input;

    // 这里装配时写进去的那份只是底稿：每一轮打模型之前，[`Gate`] 都会用当时
    // 的当前会话重写一次（见 `on_completion_call`）。
    let agent = model
        .into_agent_builder()
        .preamble(&preamble(session.get().as_ref()))
        .tool(RunCommand {
            session: session.clone(),
            events: events.clone(),
        })
        .build();

    let history: Vec<Message> = history
        .into_iter()
        .map(|(role, text)| match role {
            ChatRole::User => Message::user(text),
            ChatRole::Assistant => Message::assistant(text),
        })
        .collect();

    // `stream()` 而不是 `run()`：同一套 builder、同一个引擎、同一批钩子，
    // 差别只在模型那一侧走的是 SSE，而且多给两类观察事件（文字增量、
    // 工具参数增量）。`run()` 是把这条流折叠成一个最终答复，界面要的是
    // 一个字一个字往外冒，所以自己抽这条流。
    let mut stream = agent
        .runner(prompt)
        .history(history)
        .add_hook(Gate {
            events,
            session,
            auto: config.auto_execute,
            cancel: cancel.clone(),
        })
        .max_turns(MAX_TURNS)
        .stream()
        .await;

    // 流是惰性的：不抽干就什么都不会发生。抽出来的每一项这里都不看 ——
    // 该上屏的（模型的话、命令、待审核）都在钩子与工具里报过了，这儿只
    // 等它跑完，或者接住那个让它停下来的错。
    let mut failure = None;
    while let Some(item) = stream.next().await {
        if let Err(error) = item {
            failure = Some(error);
            break;
        }
    }

    match failure {
        None => Ok(()),
        // 用户自己按的停，不是错。钩子只在置位后 Stop，所以这里的对应关系稳。
        Some(_) if cancel.load(Ordering::Relaxed) => Ok(()),
        Some(error) => Err(anyhow::anyhow!("{error}")),
    }
}

/// 系统提示词。当前会话直接写在这儿而不是做成一个查询工具：它每一轮都
/// 重写，多一轮工具往返只是白等。
fn preamble(session: Option<&SessionTarget>) -> String {
    let current = match session {
        Some(target) => format!("- {} | {} | {}", target.name, target.login, target.platform),
        None => NO_SESSION.to_owned(),
    };
    PREAMBLE.replace("{session}", &current)
}

// ─── 审核门 ─────────────────────────────────────────────────────────────────

/// 一场运行的生命周期钩子：停止、审核、把当前会话写进上下文、把模型的话
/// 报给界面。
struct Gate {
    events: mpsc::UnboundedSender<AgentEvent>,
    /// 当前会话那一格，和工具握的是同一格。
    session: CurrentSession,
    /// 全自动模式：不再逐条送审。
    auto: bool,
    cancel: Arc<AtomicBool>,
}

impl Gate {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 一条 `run_command` 调用怎么放行。独立成方法是为了可测：
    /// [`HookContext`] 没有公开构造器，钩子方法本身在单测里搭不起来。
    async fn review(&self, args: &str) -> ToolCallAction {
        if self.cancelled() {
            return ToolCallAction::stop(STOPPED);
        }
        // 没有机器可操作就不必惊动用户：让模型直接知道这条没跑，比给一张
        // "在（空）上执行？"的卡片清楚。
        let Some(target) = self.session.get() else {
            return ToolCallAction::skip(NO_SESSION_FEEDBACK);
        };
        if self.auto {
            return ToolCallAction::Run;
        }

        // 送审要的是命令原文；目标写的是当前会话的名字，不是模型说的 ——
        // 它本来就没得选。解析失败就照原样把参数亮给用户，审核门宁可多问，
        // 不能因为参数长歪了就放行。
        let command = match serde_json::from_str::<RunCommandArgs>(args) {
            Ok(args) => args.command,
            Err(_) => args.to_owned(),
        };

        let (respond, decision) = oneshot::channel();
        if self
            .events
            .unbounded_send(AgentEvent::Approval(ApprovalRequest {
                session: target.name.clone(),
                command,
                respond,
            }))
            .is_err()
        {
            // 界面那头已经不在了，没人能批准 —— 收口。
            return ToolCallAction::stop(STOPPED);
        }

        match decision.await {
            // 卡片可以挂很久，这期间用户完全可能切到另一台机器去。批的是
            // 卡片上写的那一台，换了就不算批过。
            Ok(true) if self.session.get().is_some_and(|now| now.id == target.id) => {
                ToolCallAction::Run
            }
            Ok(true) => ToolCallAction::skip(SWITCHED_FEEDBACK),
            Ok(false) => ToolCallAction::skip(DENIED_FEEDBACK),
            Err(_) => ToolCallAction::stop(STOPPED),
        }
    }
}

/// `run_command` 的参数，审核卡片也按它读命令。字段名与
/// [`RunCommand::parameters`] 的 schema 一字对应 —— 里面没有"哪台机器"，
/// 那是当前标签页说了算的事。
#[derive(Deserialize)]
struct RunCommandArgs {
    command: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

impl AgentHook for Gate {
    /// 每次要打给模型之前两件事：看一眼"停止"（这是停止按钮最快的收口点），
    /// 然后把**此刻**的当前会话重写进系统提示词。
    ///
    /// 用户切标签页之后模型下一句话就该知道自己站在哪台机器上，靠的就是
    /// 这一下 —— [`RequestPatch`] 只作用于这一轮，不改 Agent 的底稿，也不
    /// 会在历史里堆出一串旧提示词。
    async fn on_completion_call(
        &self,
        _: &HookContext,
        _: CompletionCallEvent<'_>,
    ) -> CompletionCallAction {
        if self.cancelled() {
            return CompletionCallAction::stop(STOPPED);
        }
        CompletionCallAction::patch(
            RequestPatch::new().preamble(preamble(self.session.get().as_ref())),
        )
    }

    /// 模型每吐出一小段字就往界面送一段。流式输出就是这一条。
    ///
    /// 只有流式这条路会来（`run()` 那一侧没有增量），而我们走的正是
    /// [`AgentRunner::stream`](rig_agent::agent::AgentRunner::stream)。
    async fn on_text_delta(&self, _: &HookContext, event: TextDelta<'_>) -> ObservationAction {
        if !event.delta.is_empty() {
            _ = self
                .events
                .unbounded_send(AgentEvent::TextDelta(event.delta.to_owned()));
        }
        ObservationAction::Continue
    }

    /// 模型这一轮说完了，把规范化之后的全文报给界面，替掉刚才那串增量。
    ///
    /// 用这条而不是 `on_completion_response`：后者是**非流式**那一侧的事件，
    /// 走 `stream()` 时根本不会来。工具调用不在这儿报 —— 它们要么进审核卡片，
    /// 要么在执行完由工具自己报结果；这一轮只叫了工具没说话时，这条也不来。
    async fn on_stream_response_finish(
        &self,
        _: &HookContext,
        event: StreamResponseFinish<'_>,
    ) -> ObservationAction {
        let text = event
            .content
            .iter()
            .filter_map(|part| match part {
                AssistantContent::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if !text.trim().is_empty() {
            _ = self.events.unbounded_send(AgentEvent::Text(text));
        }
        ObservationAction::Continue
    }

    /// 命令的审核门。默认档每一条都要用户点头；全自动档直接放行。
    async fn on_tool_call(&self, _: &HookContext, event: ToolCallEvent<'_>) -> ToolCallAction {
        if event.tool_name != RunCommand::NAME {
            return ToolCallAction::Run;
        }
        self.review(event.args).await
    }
}

// ─── 工具 ───────────────────────────────────────────────────────────────────

/// 唯一的工具：在**当前这条会话**上执行一段 POSIX shell 命令。
///
/// 没有"哪台机器"这个参数，执行前现读 [`CurrentSession`] —— 这就是"助手
/// 只操作当前打开的会话"的落点：不是提示词里的一句约定，是模型根本没有
/// 别的地方可去。
///
/// 执行走 [`exec_script`]：同一条 SSH 连接上另开 session 通道、脚本喂给
/// `/bin/sh` 的标准输入、收齐输出与退出码就关 —— 和监控采集同一条路。
///
/// 出错（连不上通道、超时、眼下没有会话）不作为工具错误抛：那样模型只能
/// 看到一句笼统的分类。这里把原因放进正常输出的 `error` 字段，模型读得到
/// 细节，才知道该换条命令还是该让用户先连上。
struct RunCommand {
    session: CurrentSession,
    events: mpsc::UnboundedSender<AgentEvent>,
}

impl Tool for RunCommand {
    const NAME: &'static str = "run_command";
    type Args = RunCommandArgs;
    type Output = serde_json::Value;
    type Error = std::convert::Infallible;

    fn description(&self) -> String {
        "Run a POSIX shell command on the SSH session the user currently has open \
(the front tab) and return its stdout, stderr and exit status. The command is fed \
to /bin/sh on that remote host over a dedicated channel; the user's terminal is \
not disturbed. There is no way to target a different host — the session named in \
the system prompt is the one this runs on."
            .to_owned()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "要执行的 POSIX shell 命令（交给 /bin/sh，非交互、无 TTY）"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "最长等多少秒，默认 60，最大 300"
                }
            },
            "required": ["command"]
        })
    }

    async fn call(
        &self,
        _: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        // 现读，不是发问那一刻的快照：这一句跑之前用户可能已经换了标签页。
        let Some(target) = self.session.get() else {
            return Ok(serde_json::json!({ "error": NO_SESSION_FEEDBACK }));
        };

        let timeout = Duration::from_secs(
            args.timeout_secs
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .clamp(1, MAX_TIMEOUT_SECS),
        );

        let (payload, display, ok) = match exec_script(&target.handle, &args.command, timeout).await
        {
            Ok(output) => {
                let stdout = clip(&output.stdout);
                let stderr = clip(&output.stderr);
                let ok = output.success();
                let mut display = stdout.clone();
                if !stderr.trim().is_empty() {
                    if !display.trim().is_empty() {
                        display.push('\n');
                    }
                    display.push_str(&stderr);
                }
                (
                    serde_json::json!({
                        "stdout": stdout,
                        "stderr": stderr,
                        "exit_status": output.exit_status,
                    }),
                    display,
                    ok,
                )
            }
            Err(error) => {
                let reason = format!("{error:#}");
                (
                    serde_json::json!({ "error": reason }),
                    reason.clone(),
                    false,
                )
            }
        };

        _ = self.events.unbounded_send(AgentEvent::Command {
            session: target.name.clone(),
            command: args.command,
            output: display,
            ok,
        });
        Ok(payload)
    }
}

/// 截断到工具输出上限，按字符边界切，切过就说一声。
fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_TOOL_OUTPUT {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(MAX_TOOL_OUTPUT).collect();
    clipped.push_str("\n[output truncated]");
    clipped
}

#[cfg(test)]
mod tests {
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
}
