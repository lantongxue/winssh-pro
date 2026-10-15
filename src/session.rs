//! 会话读数类型已下沉到 `winssh-domain`（见 [`winssh_domain::session`]）：
//! `SessionTelemetry` 等纯数据都在那里。这里只做再导出，保持 `crate::session::*`
//! 旧路径不变。

pub use winssh_domain::session::*;
