//! 服务层已下沉到 `winssh-service`（见 [`winssh_service`]）：`HostService`、
//! `CredentialService`、`MonitorService`、各文件后端服务、`SettingService` 等门面
//! 都在那里——界面拿数据的唯一入口，领域模型转换与业务规则收在那一层。这里只做再
//! 导出，保持 `crate::service::*Service`、`crate::service::local_file_service` 这些
//! 旧路径不变。

pub use winssh_service::*;
