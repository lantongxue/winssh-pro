//! Clash.Meta（mihomo）订阅：把机场给的那份 yml 管起来，按需起一个 mihomo
//! 子进程，再通过它的外部控制口把代理开关接到本机上。
//!
//! ## 为什么是"起一个别人的程序"
//!
//! 这一档和组网（[`crate::easytier`]）走的是两条路。组网把 EasyTier 的**库**
//! 嵌进进程里跑；Clash 这边嵌不了 —— mihomo 没有可用的库形态，它是一个独立的
//! Go 程序。所以这里的做法是：**起进程、看住它、通过它的 RESTful API 说话**。
//! 协议解析、分流、TUN 网卡那些事全在 mihomo 里，我们只做三件事 —— 给它一份
//! 配置、在它跑偏时把它收掉、把它的状态摆到界面上。
//!
//! 这份 mihomo 由用户自己放进 `third_party/mihomo/`（见 `build.rs`），构建时
//! 拷到 exe 旁边。我们**不**代它下载、不代它升级：那是一个会改本机网络设置的
//! 程序，是哪一个版本、从哪儿来，得用户自己清楚。
//!
//! ## 分工
//!
//! - **数据**（[`subscription`] + [`crate::service::ClashSubscriptionService`]）：
//!   一条订阅 = 一个名字 + 一个来路 + **整份正文**。正文整份进库，备份同步
//!   带的就是它（快照打的是整座库，见 `crate::service::BackupService`）。
//! - **读**（[`config`]）：把正文解读成节点 / 策略组 / 分流规则给界面看，再把
//!   端口那几项盖过一遍，生成真正交给 mihomo 的那份 `config.yaml`。
//! - **说**（[`api`]）：mihomo 外部控制口的那几个端点。上游的 JSON 在这里
//!   就地译成我们自己的类型（[`snapshot`]），界面不认识 mihomo 的字段名。
//! - **跑**（[`runtime`]）：本进程里**同时只有一个** mihomo 实例。起停、端口、
//!   子进程的收尾、日志的尾巴、以及"此刻是什么状态"那份供界面订阅的快照，
//!   都在那里。
//! - **接**（[`system_proxy`]）：把系统代理指向那个混合端口，或者指向虚拟网卡
//!   （后者由 mihomo 自己建网卡，我们只负责把它的 TUN 开关拨上去）。
//!
//! ## 界面
//!
//! 侧边栏那一栏是 [`crate::ui::views::ClashView`]（订阅的增删改查），
//! 右边那块是 [`crate::ui::panels::ClashPanel`]（节点、分流规则、代理开关）。
//! 订阅表单是 [`crate::ui::dialogs::ClashFormDialog`]。

pub mod api;
pub mod config;
pub mod runtime;
pub mod snapshot;
pub mod subscription;
pub mod system_proxy;

/// 界面要的那几样，从各子模块抬到这一层 —— 外边（`src/ui/`）只认
/// `crate::clash::X` 这一种写法，和 [`crate::easytier`] 那边一个规矩。
pub use config::{GroupSummary, NodeSummary, RuleSummary, SubscriptionOutline};
pub use runtime::{Action, Phase, RuntimeState, SubscriptionRef};
pub use snapshot::{ConnectionSummary, LiveGroup, LiveNode};
pub use subscription::{ClashSubscription, ClashSubscriptionDraft, FetchedBody, SubscriptionMeta};
