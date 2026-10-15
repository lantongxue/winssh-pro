//! RDP 后端已下沉到 `winssh-rdp`（见 [`winssh_rdp`]）：IronRDP 会话状态机、位图/
//! 指针、键盘、剪贴板与文件复制通道都在那里，`RdpSettings` 经领域层再导出。这里只做
//! 再导出，保持 `crate::rdp::RdpSession`、`crate::rdp::RdpSettings` 这些旧路径不变 ——
//! 界面上那扇窗是 [`crate::ui::panels::RdpPanel`]。

pub use winssh_rdp::*;
