//! 服务层：界面拿数据的唯一入口。
//!
//! 界面只认领域模型（[`crate::host`]）和这里的异步方法，不知道 SeaORM
//! 与 SQL 的存在；业务规则（id 分配、副本命名、删分组的前置条件……）
//! 也都收在这里，数据仓只提供一条条数据操作。

pub mod forward_service;
pub mod host_service;
pub mod local_file_service;
pub mod monitor_service;
pub mod sftp_service;
pub mod ssh_service;

pub use forward_service::ForwardService;
pub use host_service::HostService;
pub use local_file_service::{LocalFileService, unique_local_path};
pub use monitor_service::{ContainerAction, MonitorService};
pub use sftp_service::{MAX_TEXT_BYTES, SftpService, TransferEvent, TransferHandle, TransferJob};
pub use ssh_service::SshService;
