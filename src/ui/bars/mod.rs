//! 条（`*Bar`）：贴着窗口边缘的固定 chrome。
//!
//! 它们的位置、尺寸与条目顺序不随内容变化，只表达“现在在哪儿”和“能做什么”，
//! 本身不放业务内容。目前只有活动栏；状态栏还内联在 [`crate::ui::Workspace`] 里，
//! 等它长出自己的状态时再挪进来。

mod activity_bar;

pub use activity_bar::{ActivityAction, ActivityBar};
