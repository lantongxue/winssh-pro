//! WinSSH Pro 领域数据层。
//!
//! 这一层只放**纯数据类型与它们的编解码**：主机配置、各类 `*Settings`、
//! 凭据、端口转发、会话读数等，是整个 workspace 依赖图的最底层。
//!
//! 铁律：本 crate **不得**依赖 `gpui` / `gpui-kit` / ui / service —— 它要能被
//! 所有后端 crate 安全地依赖而不把界面层拖进来。凡是"打开面板""调服务"这类
//! 行为都不属于这里，留在上层的根包（`winssh-pro`）或 `winssh-service`。
//!
//! 目前是脚手架阶段的占位；领域类型会在阶段 1 逐步从根包 `src/` 下沉进来。

pub mod command_history;
pub mod credential;
pub mod forward;
pub mod host;
pub mod proxy;
pub mod session;
pub mod settings;
pub mod snippet;
pub mod tray;
pub mod trusted_host;

// 领域类型里那些 `label()` 走多语言查表，`rust_i18n::t!` 会展开成本 crate 的
// `_rust_i18n_t!`，所以这一份语言表要在本 crate 根上装一次。路径相对本 crate 的
// `CARGO_MANIFEST_DIR`（`crates/winssh-domain/`）解析，`../../locales` 指回仓库根
// 那一份 `locales/`，与根包 `winssh-pro` 共用同一批 yml。`set_locale` 是 rust-i18n
// 的全局状态，根包启动时定一次，这里的 `t!` 跟着一起切。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

// 各 `*Settings` 以 gpui 的 `SharedString` 承载文本；只借这一个类型。
pub use gpui_kit::SharedString;
