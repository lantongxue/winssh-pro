//! Docker 容器管理后端已下沉到 `winssh-docker`（见 [`winssh_docker`]）：经现有
//! SSH 会话接 Engine API 的连接器、客户端、文件列举与路径纯函数都在那里。这里
//! 只做再导出，保持 `crate::docker::*` 路径可用；业务入口是
//! [`crate::service::DockerService`]，界面在右栏 Docker 页与详情弹窗。

pub use winssh_docker::*;
