//! 抽屉类组件：从窗口（或面板）边缘拉出来的一层。

mod assistant;
mod transfer_queue;

pub use assistant::{AssistantDrawer, AssistantDrawerEvent};
pub use transfer_queue::{TransferQueueDrawer, TransferQueueEvent};
