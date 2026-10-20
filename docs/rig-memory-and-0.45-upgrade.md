# rig 框架的记忆机制与升级到 0.45 的影响评估

本文由一次源码与发布记录的核查写成，目的是回答两个问题：**rig 有没有记忆系统**，
以及**我们从钉住的 0.42 升到 0.45 要动哪些地方**。所有结论都标了出处，
最后一节列出尚未核实与需要人工判断的条目。文中代码位置以本仓库路径与行号给出；本次核查只新增本文件，未改动任何生产代码。

## 摘要

| 问题 | 结论 |
| --- | --- |
| rig 有记忆机制吗 | 有，但是"会话历史托管"这一档：`ConversationMemory` 契约 + agent 自动 load/save + 可插拔的窗口/降级/摘要策略。**没有**自动提炼长期记忆、跨会话沉淀那种能力。 |
| 我们在用吗 | 没有。走的是 `.history(...)` + 我们自己的 `ai_conversation` / `ai_message` 两张表。而"传了 history 就整体绕过 memory"正是 rig 的语义，所以即便挂上 backend 也不会生效。 |
| 升级到 0.45 的代价 | 不是小版本升级：中间隔着 0.43 那次架构重写。必改集中在 `crates/winssh-assistant/src/agent.rs`，粗估 300–500 行针对性改动加一轮兼容网关回归。 |
| 升级的必要性 | **低**。0.45 新增的是模型目录、citations、成本核算、rig-memory 重写、Bevy 编码内核；我们没有一项功能非它不可。风险主要来自行为变严（未知 finish reason、截断流、不支持的选项都会失败整个 run），而我们的主力是 DeepSeek / 智谱 / 千问 / 豆包这类兼容网关。 |

## 一、核实依据

| 来源 | 用途 |
| --- | --- |
| `Cargo.toml` L243-L254（含注释） | 我们钉的是 `rig-agent = "0.42"`、`rig-core = "0.42"`（`default-features = false`，`["reqwest","rustls"]`），`Cargo.lock` 解析为 0.42.0 |
| 本地 cargo registry 解包源码：`registry/src/<index-hash>/rig-core-0.42.0/src/memory.rs`（504 行全文） | 0.42 记忆契约的一手定义 |
| 同一 registry 源码目录下 `rig-agent-0.42.0/src/agent/{builder.rs,completion.rs,runner.rs,prompt_request/*}` | 记忆在运行循环里的挂接点与绕过语义 |
| docs.rs：`rig-memory` 0.42.0 与 latest（0.45.0）、`rig-agent` 0.45.0（crate / `agent` / `agent::hook` / `AgentHook` / `DispatchEvent` / `DispatchAction` / `ObservationAction` / `AgentBuilder` / `AgentRunner`）、`rig-core` 0.45.0（`tool` / `tool::contextual::Tool` / `completion` / `completion::message::Message` / `providers::openai` / `providers::anthropic`） | 0.45 目标态 API 的签名与文档 |
| rig 仓库 `CHANGELOG.md`、`MIGRATING.md`（0.42→0.43 / 0.43→0.44 / 0.44→0.45 三节全文） | 破坏性变更的逐条出处与迁移草图 |
| crates.io sparse index：`rig-memory`、`rig-core`、`rig-agent` 的 0.45.0 条目 | 版本可用性与特性表 |
| `crates/winssh-assistant/src/agent.rs`、`thinking.rs`、`tests/assistant/agent.rs` | 我们的实际触点清单 |

`rig-memory` 已发布 0.1.0 → 0.45.0；0.42.0 与 0.43/0.44/0.45 均存在，版本与 rig 主版本同步。
0.43.0 发布于 2026-09-30，0.44.0 发布于 2026-10-07，**0.45.0 发布于 2026-10-19**（本文写作当天）。

## 二、rig 0.42 的记忆机制

### 契约层 `rig_core::memory`（`pub mod memory`，无特性门控）

- `trait ConversationMemory`：`load(conversation_id)` / `append(conversation_id, messages)` /
  `clear(conversation_id)`。文档写明调用时机是**发 prompt 前 load、一轮成功后 append**，
  并提醒 `append` 要便宜 —— 它内联在返回响应之前跑。
- `InMemoryConversationMemory`：唯一的内置后端，`Arc<Mutex<HashMap<String, Vec<Message>>>>`，
  **纯进程内存、重启即失**，定位是测试/示例/短命 agent。`with_filter(closure)` 在每次 load 上做整形。
- `DemotionHook` + `NoopDemotionHook`：被策略踢出活动窗口的消息的旁路出口，把"截断"变成"降级"。
- `Compactor`：反向操作 —— 把被驱逐的前缀连同上一次的 `carry_over` 压成一条 summary，
  **再拼回活动历史头部**（滚动摘要）。
- `MemoryError::{Backend, Policy, Internal}`；`Arc<M>` / `Box<M>` 有转发 impl。

### 运行层 `rig-agent`

- 装配：`AgentBuilder::memory<B: ConversationMemory>` + `AgentBuilder::conversation(id)`。
- 每请求：`PromptRequest::conversation(id)`（覆盖默认）、`PromptRequest::without_memory()`。
- `AgentRunner::resolve_history_and_memory()`：`chat_history` 为 `Some` 时**直接返回 `(None, None)`**，
  既不 load 也不 save。
- 保存：`Done` 分支调 `append_run_messages(memory_handle, response.messages)`，
  阻塞 `run()` 与 `stream()` 共用；append 失败只 `tracing::warn!`，最终响应照发。
- 三条静默语义：① 带 `.history(...)` 即绕过 memory；② memory 与 conversation_id 缺任一
  "silently bypassed"，不报错；③ 只有成功的那一轮会 append。

### 配套 crate `rig-memory`（0.42 那一档）

提供的是**整形策略与适配器，不是持久化后端**：`NoopMemoryPolicy`、`SlidingWindowMemory`、
`TokenWindowMemory`、`HeuristicTokenCounter`、`PolicyMemory`、`DemotingPolicyMemory`、
`CompactingMemory`、`TemplateCompactor`/`TextSummary`、`IntoFilter`。所有 sliding 策略会丢掉
开头的孤儿 tool-result（provider 普遍拒绝无配对的 tool result）。**只有内存 store，没有 SQL/文件实现。**

### 相邻机制（不要和记忆混淆）

`AgentBuilder::context(&str)` 静态上下文文档；`AgentBuilder::dynamic_context(samples, index)`
每次调模型前做向量检索并注入（语义记忆要自己用 `vector_store` 搭）；`ToolContext` 是每请求侧信道，
模型看不见，不是记忆。

### 我们的现状

- `crates/winssh-assistant/src/agent.rs` L618-L620 用 `agent.runner(prompt).history(history)`，
  历史由 `ai_conversation` / `ai_message` 两张表经服务层供给，图片按 `HISTORY_IMAGE_LIMIT = 6`（L143）裁剪。
- 全仓库没有 `.memory(`、`ConversationMemory`、`.conversation(` 的任何使用。
- **0.45 变更**：`rig_core::memory` 整模块被删（`ConversationMemory` / `InMemoryConversationMemory` /
  `MessageFilter` / `DemotionHook` / `Compactor` / `MemoryError` 均无兼容层），换成
  `rig_core::conversation::Conversation` + `rig_memory` 的 `Store` / `MemoryStore` / `DirStore` /
  `AsyncStore` / `load` / `save` / `CompactionPolicy` / `Summarizer` / `recent::Recent`；
  `AgentBuilder::memory(store: impl AsyncStore).conversation(log_name)`，错误是
  `PromptError::Memory { error, response }`。**对我们零冲击**（本来没用），但"以后接 SeaORM 实现
  `ConversationMemory`"这个方案作废，要改成实现 `AsyncStore`。

## 三、0.42 → 0.45 的改动清单

### 3.1 Cargo / 工具链

| 项 | 现状 | 0.45 | 处置 |
| --- | --- | --- | --- |
| MSRV | CI `RUST_TOOLCHAIN: 1.99.0`（`release.yml` L56） | 0.43 要 1.95.0，0.45 要 **1.97.1** | 不用动 |
| `rig-core` 特性 | `["reqwest","rustls"]` | 名字仍在，但 `reqwest = ["dep:rig-reqwest"]`、`rustls = ["rig-reqwest?/rustls", …]`，默认是 `["derive","rustls"]` | 特性照写，注释要补"reqwest 实体已拆成 `rig-reqwest`" |
| `rig-agent` → `rig-core` | `default-features = false` | 仍是 `default-features = false` | 根 `Cargo.toml` L249-L252 那段"HTTP 后端要由我们这份点亮"的理由**不变** |
| serde_json | — | 0.44 起 rig-core 打开 `preserve_order` + `float_roundtrip`，**Cargo 在整个二进制里统一** | 需回归所有"序列化后比字符串/字节"的地方：备份同步比对、mihomo 配置 diff、入库的 JSON 字段 |

### 3.2 客户端与模型构造（`agent.rs` L493-L528、L532-L534）

0.43 删掉了 `Client<Ext, H>`、`ClientBuilder`、`CompletionClient` 这类能力 trait 与 `AgentModelExt`，
客户端机制塌成"Provider 自己持有 transport"。

| 现在 | 0.45 |
| --- | --- |
| `openai::CompletionsClient::builder().api_key(k).base_url(u).build()?` | `openai::OpenAIConfig::with_key(&dialect, k).with_base_url(u).with_route(route).connect(http).completion(model)`，或 `openai::OpenAI::new(k).chat(model)` |
| `openai::Client::builder()…`（Responses） | `openai::OpenAI::new(k).responses(model)` |
| `anthropic::Client::builder()…` | `anthropic::Anthropic::new(k)` / `AnthropicConfig::with_key(&dialect, k)`；`AnthropicConfig` 字面量要补 `addressing: Addressing::Body` |
| `client.completion_model(m)` | `client.completion(m)`（返回 `Model<Wire>`） |
| `drive<M: CompletionModel + 'static>` | **去泛型**，收 `DynModel<Completion>`（`CompletionModel` trait 已删） |
| `use rig_core::client::completion::CompletionClient as _;` | 删掉这个 import |

DeepSeek / 智谱 / 千问等 OpenAI 兼容家在 0.43 起统一走 `providers::<vendor>::from_env()` 或
`OpenAIConfig::with_key(&dialect, …)`，有自己的 `Dialect` 常量 —— 有机会替掉我们自己维护的
`ProviderProtocol` 表，但要看这些 dialect 是否覆盖我们的 quirks（见第五节）。

### 3.3 Agent 装配与运行（`agent.rs` L549-L629）

- `model.into_agent_builder()` → `AgentBuilder::new(model)`；`AgentBuilder` 不再有类型参数
  （`let builder: AgentBuilder = …`）。
- 仍在的 setter：`preamble`、`tool`、`max_tokens`、`additional_params(Value)`、`build`；
  runner 侧 `history`、`add_hook`、`max_turns`、`stream`、`preamble`、`without_memory`。
- `agent.runner(prompt)` → `agent.prompt(prompt)`（0.43 删 `Agent::runner` 与 `PromptRequest`，setter 合进 `AgentRunner`）。
- `AgentRunner` 的 `additional_params` 被拆成 `merge_additional_params(Map)` /
  `replace_additional_params(Value)` / `without_additional_params`，builder 上仍是 `additional_params(Value)`。
- 合并顺序变了（0.44）：wire 编码 → 映射后的 typed options → provider options → `additional_params`，
  逐 key 合并，**`additional_params` 现在覆盖 typed 字段**。`thinking::request_params` 这条路可继续用，
  但每条协议的写法要重新实测。
- 可选收益：`AgentBuilder::reasoning(..)` / `options(GenerationOptions)` /
  `provider_option::<AnthropicExt|OpenAiExt>()` 是官方思考强度通路，可替掉 thinking 的手写 JSON；
  代价是默认策略 `OnUnsupported::Error` —— 不认这档的模型会**直接报错**，得显式 `Ignore`，
  而这正是我们现在按模型勾 `efforts` 的原因。
- 0.44 起 `StreamingError` 并进 `PromptError`（变体去 stutter：`Provider` / `Memory` / `MaxTurns` / `Cancelled`），
  `PromptResponse::messages` 从 `Option<Vec<Message>>` 变成必填 `Vec<Message>`，
  `output` 字段变 `output()` 方法；`PromptError`、`MultiTurnStreamItem`、`AssistantContent`、
  `FinishReason`、`ProviderError` 全成 `#[non_exhaustive]`（我们现有 match 都带 `_ =>`，风险低）。
- **行为变严（不改代码也会变）**：0.44 未知 finish reason / 被过滤内容 / 上报失败 → run 失败
  （可用 `accept_unknown_finish_reasons` 关掉）；0.43 截断流 → `ProviderError::Truncated`（可重试）；
  0.45 带内错误也可判可重试，可能在吐出一半之后重试。

### 3.4 钩子：`Gate` 是主战场（`agent.rs` L960-L1063）

**0.43 删除 `on_tool_call` / `on_tool_result` / `on_completion_response` /
`on_stream_response_finish`，以及 `ToolCallAction` / `ToolResultAction` / `ToolResultEvent` /
`ToolCall` 事件**。审核门搬到 dispatch 边界：

```rust
async fn on_dispatch(&self, _: &HookContext, event: DispatchEvent<'_>) -> DispatchAction
```

| 现在 | 0.45 |
| --- | --- |
| `event.tool_name: &str` | `event.tool_name(): Option<&str>` |
| `event.args: &str` | `event.tool_args(): Option<&str>`（我们的 `review` / `review_transfer` / `review_local` / `review_shell` 本来就收 `&str`） |
| `ToolCallAction::Run` | `DispatchAction::Proceed` |
| `ToolCallAction::skip(reason)` | `DispatchAction::skip(reason)`（同样把"跳过"作为 tool result 回给模型） |
| `ToolCallAction::stop(reason)` | `DispatchAction::stop(reason)`（以 `Cancelled` 报告取消整个 run） |
| — | 新增 `DispatchAction::Patch(EffectKind)` / `rewrite_tool_args(..)` 可改参数再放行 |

- ⚠️ **最容易埋回去的坑**：`on_dispatch` / `on_outcome` 由 `AgentHook::observes(StepEventKind)` 门控。
  默认对 `CompletionDispatch` / `ToolDispatch` 为 true，但谁为了削增量噪音覆写 `observes` 而漏掉
  `StepEventKind::ToolDispatch`，审核门会**静默失效**、所有命令直接跑。这条要写进代码注释。
- `on_completion_call`（每轮重写 preamble）在 0.45 仍是这个名字，事件类型 `CompletionCallEvent`、
  动作 `CompletionCallAction::{patch, stop}`、`RequestPatch` 都在；注意 0.45 里 hook 的 histories 与
  `RequestPatch::history` 都变成 `Arc<Message>`。
- `on_text_delta` / `on_reasoning_delta` / `on_tool_call_delta` / `ObservationAction::Continue` 都在。
  0.44 起 `on_tool_call_delta` **每个片段来一次并带 `aggregated`**，`ToolCallDelta` 失去 `call_id`
  （我们现在忽略字段，安全）；`ReasoningDelta` 的 aggregate 按 correlator 分块。
- `on_model_turn_finished` 还在，两处要改：
  1. `event.usage.output_tokens` 从 `u64` 变 **`Option<u64>`**（0.43），`tokens_per_second(tokens: u64, …)`
     （L1869）签名要跟着改；`Usage::new()` → `Usage::default()`、`has_values()` → `is_reported()`、
     `Usage` 不再 `Eq`。
  2. `AssistantContent::Reasoning(..)` 现在装 `Sealed` + `Issuer`，`display_text()` 不保证还在 ——
     改用 `open(&issuer)` / `AssistantMessage::text()`（0.45 把 `transcript::assistant_text_from_choice`
     换成 `AssistantMessage::text()`）。
- 0.45 语义：每次 dispatch **恰好一次 `on_outcome`**，但 `Ok` 只代表"完整响应到了"，**不代表这一轮被接受**；
  落库/计费逻辑只能放 `on_model_turn_finished`（我们现在报 `TurnEnd` 正是这条，方向对）。

### 3.5 消息、图片、历史（`agent.rs` L655-L718、`image.rs`、测试）

- `Message::user(..)` / `Message::assistant(..)` / `Message::User { content }` /
  `UserContent::image_base64(..)` 继续可用；`Message::Assistant` 变成
  `Message::Assistant(AssistantMessage)`（带 origin/stop 与 provenance）。我们只造纯文本
  assistant 轮，不受影响。
- **`DocumentSourceKind::String` 被 `DocumentData::{Text(String), File(DocumentSourceKind)}` 取代**，
  `ContentFormat` 删除，`DocumentSourceKind::try_into_inner` 也删。
  `tests/assistant/agent.rs` L501 那句 `DocumentSourceKind::Base64(..)` 断言要按新形状重写。
  0.44 一并把 media 构造器改成名字自说明：`UserContent::audio_base64` / `video_base64` / `document_text`。
- 0.43 起 tool call 带必填 `CallId`（`Provider` / `Local`）与 `ToolName`，`ToolCallId` 删除。
  我们自建历史不带工具轮次，暂时碰不到；真带就要过 `conversation::pairing::{validate, repair}`（0.45）。
- 0.44 的 `history::adapt` 会跳过失败的轮次、同模型轮原样重放、异模型轮用 canonical 字段重建；
  0.45 静默变化里还有一条：老 `"type":"string"` 的文档 JSON 会读成纯 text。
  需确认 `ai_message` / `ai_image` 表里没有直接存 rig 的 `Message` serde 形态（据现有代码是存我们自己的
  `ChatMessage`，判断为安全，审查时请复核）。

### 3.6 工具实现：基本不用动（8 个 `impl Tool`）

0.45 只留一份 contextual `Tool`（`rig_core::tool::Tool`），要求：

```rust
const NAME: &'static str;
type Args: for<'de> Deserialize<'de> + Send + Sync;
type Output: IntoToolOutput;
type Error: Error + Send + Sync + 'static;
fn description(&self) -> String;
fn parameters(&self) -> Value;
async fn call(&self, context: &mut ToolContext, args: Self::Args) -> Result<Self::Output, Self::Error>;
// 可选：annotations()、footprint()、map_error()
```

和我们现在的写法一致（`PortableTool` / `PortableToolEmbedding` 那条线在 0.45 被删）。
0.43 起 `ToolContext` 的值必须 typed + 可序列化（derive `ContextValue`），`insert/get/require`
返回 `Result`；我们只是忽略这个参数，无影响。

新增的 `annotations()` / `footprint()` 是声明式的"这工具读什么、改什么、要不要审批"，
rig 的 approval / plan-mode / 并行调度按它走 —— 有机会把 `local::review` / `shell::parse` 里的
命令式判断升级成声明。建议当第二步单独做，不要混进升级本身。

### 3.7 其它要顺手核对的

- 0.43 起 `agent.prompt(p)` 返回 `PromptResponse`（不再是 `String`）。
- Anthropic / Bedrock 的 `input_tokens` 现在含缓存读写与写入，Gemini 的 `output_tokens` 含 thoughts、
  `input_tokens` 含 hosted-tool prompt tokens —— 界面上 tokens/s 与用量统计的口径会变
  （Gemini 那条会偏高）。
- 依赖树瘦身：`serenity` / `rayon` / `as-any` / `eventsource-stream` 被移除；`discord-bot` 特性删除；
  `rig-agent` 的 `rmcp` / `discord-bot` 特性删除（我们都没开，`derive` 也关着）。
- 0.45 引入 `Handlers` 内联表替掉 effect bus（`AgentBuilder::over_bus` → `over_handlers`），
  以及 `TurnPolicy`、`PendingToolCall::{Execute, Malformed}`、`run.answer(..)`、`run::project`
  这些 sans-IO 细节 —— 我们走的是 `stream()` 高层入口，只有在直接操 `AgentRun` 时才需要碰。
- 持久化 run 的 `RUN_FORMAT` 到 0.45 是 5（0.44 是 2），没有升级 shim。我们不落盘 rig 的 run 状态，无冲击。

## 四、改动量与建议顺序

| 步 | 内容 | 粗估 |
| --- | --- | --- |
| 1 | 只做编译迁移：3.1 版本与特性注释、3.2 客户端构造与去泛型、3.3 `AgentBuilder::new` / `agent.prompt`、3.4 `on_dispatch` 重写、3.5 usage/图片断言 | `crates/winssh-assistant/src/agent.rs` 为主，300–500 行 + 测试同步 |
| 2 | 兼容网关回归：DeepSeek / 智谱 / 千问 / 豆包 各跑「带工具 / 带思考 / 中途停止 / 长回答被截断」四类对话，验证 finish_reason 与截断策略没把 run 打死 | 需要人工实机，禁自启动截图（项目规则） |
| 3 | 收益项单独做：typed `GenerationOptions` / `provider_option` 替 thinking 表、`Tool::annotations` 替审核白名单、`run::project` 把工具轮次写进 `ai_message` | 与升级解耦 |

审查时请重点判两件事：**是否值得升**（我的评估是没有刚性需求，0.45 发布仅一天），
以及**第 1 步能否在不改审核门语义的前提下完成**（`on_dispatch` + `observes` 那条是本地的行为回归高发点）。

## 五、尚未核实 / 需要人工判断

1. `Route` 枚举的变体名（0.45 `openai::wire::Route` 只确认存在，未逐个核对 `Chat` / `Responses` 拼写）。
2. 0.45 `Reasoning` / `AssistantMessage` 读思考正文的确切方法名（`display_text()` 是否保留未证实），
   以及 `ModelTurnFinished::content` 的确切类型（是否 `&[AssistantContent]`）。
3. 各家 `Dialect` 是否覆盖我们 `ThinkingSwitch` 的三种写法（`enable_thinking` / `thinking.type` / 不发开关），
   以及 OpenAI 官方对未知参数直接 400 的处理是否已由 dialect quirks 兜住。
4. `rig_agent::tool::{Tool, ToolContext}` 这条 import 路径在 0.45 是否仍有效（0.45 `rig_agent::tool`
   模块存在，描述为"tool authoring…"，推测是再导出 `rig_core::tool`，未编译验证）。
5. 全部结论来自文档、CHANGELOG、MIGRATING 与 docs.rs 签名，**没有实际编译过一次 0.45**；
   迁移草图官方自述为"source-checked, not compiled"。真要动手前应先做一次
   `cargo update -p rig-core --precise 0.45.0` 之类的试编译，把错误清单拿回来核对本文覆盖率。
6. 0.45 的 `rig-ecs`（Bevy 0.20 新内核）与 `rig-harness` / `rig-tools` / `rig-basics` / `rig-tui`
   那套编码 agent 插件只做了存在性确认，与本项目无关，未评估。
