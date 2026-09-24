//! 条（`*Bar`）：贴着边缘的固定 chrome。
//!
//! 它们的位置、尺寸与条目顺序不随内容变化，只表达"现在在哪儿"和"能做什么"，
//! 本身不放业务内容：[`ActivityBar`] 是窗口最左边那条全局导航，
//! [`CommandCenterBar`] 是标题栏正中那一段入口，
//! [`SessionInfoBar`] 与 [`SessionStatusBar`] 是终端的上下两沿。
//!
//! 后两条贴的是面板的边缘而不是窗口的，但性质是一样的：它们是那个面板的
//! chrome，内容再怎么滚、终端再怎么刷屏，它们都待在原处。

mod activity_bar;
mod command_center;
mod session_info;
mod session_status;

pub use activity_bar::{ActivityAction, ActivityBar};
pub use command_center::CommandCenterBar;
pub use session_info::SessionInfoBar;
pub use session_status::SessionStatusBar;

// 桌面面板（RDP/VNC）信息栏复用终端状态栏那一套"标签 + 定宽值 + 配色"的格子，
// 免得同一个数字在两处画成两副样子（见 [`session_status::link_stats_group`]）。
pub(crate) use session_status::link_stats_group;
