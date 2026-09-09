//! rig 那一侧：把一次用户提问跑成一场多轮的 Agent 运行。
//!
//! 结构故意压得很平：一个工具（[`RunCommand`]，在已连接的服务器上执行命令）、
//! 一个钩子（[`Gate`]，人工审核 + 停止 + 把进展报给界面）、一个入口
//! （[`run`]）。多轮循环、工具调度、无效调用恢复都归 rig 的
//! [`rig_agent::AgentRunner`]，这里不重写它。
//!
//! 界面与运行之间只有一条事件通道（[`AgentEvent`]）：模型说的话、命令的
//! 执行结果、待审核的命令都从这里出去。审核是一问一答 —— 事件里带一个
//! `oneshot`，界面按下"批准/拒绝"把答案送回来，钩子在 [`Gate::on_tool_call`]
//! 里等着它。全自动模式跳过这一问。
//!
//! **助手不碰本机文件。** 这不是一句提示词的事：它根本没有任何本地文件
//! 工具可调，唯一的工具走的是 SSH 连接上另开的 exec 通道
//! （[`crate::ssh::exec_script`]），和终端、SFTP 互不干扰。
//!
//! reqwest 要 tokio 的 reactor，所以 [`run`] 必须经由 [`crate::db::run`]
//! 派到应用级 tokio runtime 上，界面侧 `await` 到的只是个信封。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::channel::{mpsc, oneshot};
use gpui_kit::SharedString;
use rig_agent::agent::{
    CompletionCallAction, CompletionCallEvent, CompletionResponseEvent, ObservationAction,
    ToolCall as ToolCallEvent, ToolCallAction,
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

/// 系统提示词模板。`{sessions}` 替换成已连接会话的清单。
///
/// 是常量不是 i18n 条目：这是说给模型听的，不该跟着界面语言变 ——
/// 里面已经写明"用用户提问的语言回答"。
const PREAMBLE: &str = "\
You are the operations assistant inside WinSSH Pro, an SSH client.

You can run commands on the user's ALREADY CONNECTED servers through the \
`run_command` tool. Currently connected sessions (session_id | name | login | platform):
{sessions}

Rules:
- Commands are executed by a non-interactive POSIX /bin/sh without a TTY. Never \
run interactive or long-lived programs (top, vim, tail -f, watch); use one-shot \
alternatives (ps, df, tail -n 100).
- You have NO access to the user's local machine (the SSH client itself) and no \
local file tools. If asked to read or write local files, say plainly that you \
cannot.
- By default every command you run is shown to the user for approval first; a \
declined command was NOT executed. Explain risky operations (deleting data, \
restarting services, changing firewalls) before running them.
- When there is no connected session, help with advice and say the user must \
connect to a host first before you can run anything.
- Answer in the language the user writes in.";

/// 会话清单为空时替进 `{sessions}` 的那一行。
const NO_SESSIONS: &str = "(no connected sessions right now)";

/// 用户在审核里拒绝之后回给模型的话。
const DENIED_FEEDBACK: &str =
    "The user declined to run this command. It was NOT executed. Ask or adjust before retrying.";

/// 用户按下"停止"之后收口用的原因。
const STOPPED: &str = "The user stopped this run.";

/// 一条已连接的会话在助手眼里的样子：够模型挑对机器，够工具拿到通道。
///
/// `handle` 是那条 SSH 连接本体的共享句柄 —— SSH 是多路复用的，命令走的是
/// 连接上**另开**的一条 exec 通道，用户正在敲的终端不受影响。
#[derive(Clone)]
pub struct SessionTarget {
    /// 会话 id（主机 id）。模型在 `run_command` 里靠它点名。
    pub id: SharedString,
    /// 显示名，审核卡片与执行卡片上写的是它。
    pub name: SharedString,
    /// `user@hostname`，帮模型分清同名主机。
    pub login: SharedString,
    /// 操作系统标识，模型选命令的写法要看它。
    pub platform: SharedString,
    pub handle: Arc<Handle<SshHandler>>,
}

/// 运行过程中发给界面的一条事件。
pub enum AgentEvent {
    /// 模型这一轮说的话（一轮里的全部文字部分）。
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
    /// 按下"发送"那一刻已连接的会话快照。
    pub sessions: Vec<SessionTarget>,
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
        sessions,
        history,
        prompt,
        events,
        cancel,
    } = input;
    let sessions = Arc::new(sessions);

    let agent = model
        .into_agent_builder()
        .preamble(&preamble(&sessions))
        .tool(RunCommand {
            sessions: sessions.clone(),
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

    let outcome = agent
        .runner(prompt)
        .history(history)
        .add_hook(Gate {
            events,
            auto: config.auto_execute,
            cancel: cancel.clone(),
        })
        .max_turns(MAX_TURNS)
        .run()
        .await;

    match outcome {
        Ok(_) => Ok(()),
        // 用户自己按的停，不是错。钩子只在置位后 Stop，所以这里的对应关系稳。
        Err(_) if cancel.load(Ordering::Relaxed) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// 系统提示词。会话清单直接写在这儿而不是做成一个查询工具：
/// 清单在按下"发送"那一刻就定了，多一轮工具往返只是白等。
fn preamble(sessions: &[SessionTarget]) -> String {
    let roster = if sessions.is_empty() {
        NO_SESSIONS.to_owned()
    } else {
        sessions
            .iter()
            .map(|target| {
                format!(
                    "- session_id: {} | {} | {} | {}",
                    target.id, target.name, target.login, target.platform
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    PREAMBLE.replace("{sessions}", &roster)
}

// ─── 审核门 ─────────────────────────────────────────────────────────────────

/// 一场运行的生命周期钩子：停止、审核、把模型的话报给界面。
struct Gate {
    events: mpsc::UnboundedSender<AgentEvent>,
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
        if self.auto {
            return ToolCallAction::Run;
        }

        // 送审要的两样：命令原文和目标会话名。解析失败就照原样把参数
        // 亮给用户 —— 审核门宁可多问，不能因为参数长歪了就放行。
        let (session, command) = match serde_json::from_str::<RunCommandArgs>(args) {
            Ok(args) => (SharedString::from(args.session_id), args.command),
            Err(_) => (SharedString::default(), args.to_owned()),
        };

        let (respond, decision) = oneshot::channel();
        if self
            .events
            .unbounded_send(AgentEvent::Approval(ApprovalRequest {
                session,
                command,
                respond,
            }))
            .is_err()
        {
            // 界面那头已经不在了，没人能批准 —— 收口。
            return ToolCallAction::stop(STOPPED);
        }

        match decision.await {
            Ok(true) => ToolCallAction::Run,
            Ok(false) => ToolCallAction::skip(DENIED_FEEDBACK),
            Err(_) => ToolCallAction::stop(STOPPED),
        }
    }
}

/// `run_command` 的参数，审核卡片也按它读命令。字段名与
/// [`RunCommand::parameters`] 的 schema 一字对应。
#[derive(Deserialize)]
struct RunCommandArgs {
    session_id: String,
    command: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

impl AgentHook for Gate {
    /// 每次要打给模型之前看一眼"停止"。这是停止按钮最快的收口点。
    async fn on_completion_call(
        &self,
        _: &HookContext,
        _: CompletionCallEvent<'_>,
    ) -> CompletionCallAction {
        if self.cancelled() {
            return CompletionCallAction::stop(STOPPED);
        }
        CompletionCallAction::Continue
    }

    /// 模型每说完一轮，把文字部分报给界面。工具调用不在这儿报 ——
    /// 它们要么进审核卡片，要么在执行完由工具自己报结果。
    async fn on_completion_response(
        &self,
        _: &HookContext,
        event: CompletionResponseEvent<'_>,
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

/// 唯一的工具：在一条已连接的会话上执行一段 POSIX shell 命令。
///
/// 执行走 [`exec_script`]：同一条 SSH 连接上另开 session 通道、脚本喂给
/// `/bin/sh` 的标准输入、收齐输出与退出码就关 —— 和监控采集同一条路。
///
/// 出错（连不上通道、超时、找不到会话）不作为工具错误抛：那样模型只能
/// 看到一句笼统的分类。这里把原因放进正常输出的 `error` 字段，模型读得到
/// 细节，才知道该换台机器还是换条命令。
struct RunCommand {
    sessions: Arc<Vec<SessionTarget>>,
    events: mpsc::UnboundedSender<AgentEvent>,
}

impl Tool for RunCommand {
    const NAME: &'static str = "run_command";
    type Args = RunCommandArgs;
    type Output = serde_json::Value;
    type Error = std::convert::Infallible;

    fn description(&self) -> String {
        "Run a POSIX shell command on one of the user's connected SSH sessions and \
return its stdout, stderr and exit status. The command is fed to /bin/sh on the \
remote host over a dedicated channel; the user's terminal is not disturbed."
            .to_owned()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "在哪条已连接会话上执行，取系统提示里清单中的 session_id"
                },
                "command": {
                    "type": "string",
                    "description": "要执行的 POSIX shell 命令（交给 /bin/sh，非交互、无 TTY）"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "最长等多少秒，默认 60，最大 300"
                }
            },
            "required": ["session_id", "command"]
        })
    }

    async fn call(
        &self,
        _: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let Some(target) = self
            .sessions
            .iter()
            .find(|target| target.id.as_ref() == args.session_id)
        else {
            let known = self
                .sessions
                .iter()
                .map(|target| target.id.as_ref())
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(serde_json::json!({
                "error": format!(
                    "no connected session with id {:?}; connected sessions: [{known}]",
                    args.session_id
                ),
            }));
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
    use futures::StreamExt as _;

    /// 工具端到端：命令要一字不差地进远端 `/bin/sh` 的标准输入，输出与
    /// 退出码收回来，同时给界面报一条 `Command` 事件。用的是进程内那台
    /// 假 sshd（[`crate::ssh::exec::fake`]），不碰网络。
    #[test]
    fn run_command_reaches_the_remote_shell_and_reports_back() {
        crate::db::runtime().block_on(async {
            let (handle, seen) = crate::ssh::exec::fake::connect("total 0\n", "", true).await;
            let (tx, mut rx) = mpsc::unbounded();
            let tool = RunCommand {
                sessions: Arc::new(vec![SessionTarget {
                    id: "web-01".into(),
                    name: "web-01".into(),
                    login: "root@10.0.0.1".into(),
                    platform: "Ubuntu".into(),
                    handle,
                }]),
                events: tx,
            };

            let payload = tool
                .call(
                    &mut ToolContext::new(),
                    RunCommandArgs {
                        session_id: "web-01".into(),
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

    /// 点名一条不存在的会话不该 panic 也不该抛错：模型要在 `error` 字段里
    /// 读到"有哪些可选"，才能自己纠偏。
    #[test]
    fn an_unknown_session_reports_choices_instead_of_failing() {
        crate::db::runtime().block_on(async {
            let (handle, _) = crate::ssh::exec::fake::connect("", "", true).await;
            let (tx, _rx) = mpsc::unbounded();
            let tool = RunCommand {
                sessions: Arc::new(vec![SessionTarget {
                    id: "web-01".into(),
                    name: "web-01".into(),
                    login: "root@10.0.0.1".into(),
                    platform: "Ubuntu".into(),
                    handle,
                }]),
                events: tx,
            };

            let payload = tool
                .call(
                    &mut ToolContext::new(),
                    RunCommandArgs {
                        session_id: "db-02".into(),
                        command: "uptime".into(),
                        timeout_secs: None,
                    },
                )
                .await
                .unwrap();
            let error = payload["error"].as_str().unwrap();
            assert!(error.contains("web-01"), "错误里要列出可选的会话：{error}");
        });
    }

    /// 审核门：批准放行、拒绝换成 Skip、全自动直接放行、按了停止就 Stop。
    #[test]
    fn the_gate_translates_decisions_into_actions() {
        crate::db::runtime().block_on(async {
            const ARGS: &str = r#"{"session_id":"web-01","command":"rm -rf /tmp/x"}"#;

            // 全自动：不送审，直接放行。
            let (tx, mut rx) = mpsc::unbounded();
            let gate = Gate {
                events: tx,
                auto: true,
                cancel: Arc::new(AtomicBool::new(false)),
            };
            assert!(matches!(gate.review(ARGS).await, ToolCallAction::Run));
            assert!(rx.try_next().is_err(), "全自动模式不该发审核事件");

            // 手动档：批准 → Run，拒绝 → Skip。审核事件里带着命令原文。
            for (approve, want_run) in [(true, true), (false, false)] {
                let (tx, mut rx) = mpsc::unbounded();
                let gate = Gate {
                    events: tx,
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

            // 停止按钮压倒一切。
            let (tx, _rx) = mpsc::unbounded();
            let gate = Gate {
                events: tx,
                auto: true,
                cancel: Arc::new(AtomicBool::new(true)),
            };
            assert!(matches!(gate.review(ARGS).await, ToolCallAction::Stop(_)));
        });
    }
}
