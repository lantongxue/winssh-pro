//! 主机数据模型已下沉到 `winssh-domain`（见 [`winssh_domain::host`]）：`Host`、
//! `HostKind`、`AuthMethod`、`MonitorIntervals`、`HostDraft`、`HostStatus`、
//! `HostGroup` 与 tags 的编解码都在那里。这里只做再导出，保持 `crate::host::Host`
//! 这样的旧路径不变 —— 主机是"怎么连"的领域数据，落库归
//! [`crate::service::HostService`]，"打开哪种会话窗"的胶水在
//! [`crate::ui::views::HostExplorerView`]。

pub use winssh_domain::host::*;
