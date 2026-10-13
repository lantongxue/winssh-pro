//! 服务层：界面拿数据的唯一入口。
//!
//! 界面只认领域模型（[`crate::host`]）和这里的异步方法，不知道 SeaORM
//! 与 SQL 的存在；业务规则（id 分配、副本命名、删分组的前置条件……）
//! 也都收在这里，数据仓只提供一条条数据操作。

pub mod assistant_service;
pub mod backup_service;
pub mod clash_subscription_service;
pub mod command_history_service;
pub mod credential_service;
pub mod file_service;
pub mod forward_service;
pub mod ftp_service;
pub mod host_service;
pub mod local_file_service;
pub mod monitor_service;
pub mod s3_service;
pub mod serial_service;
pub mod setting_service;
pub mod sftp_service;
pub mod snippet_service;
pub mod ssh_service;
pub mod telnet_service;
pub mod trusted_host_service;
pub mod webdav_service;

pub use assistant_service::AssistantService;
pub use backup_service::BackupService;
pub use clash_subscription_service::ClashSubscriptionService;
pub use command_history_service::CommandHistoryService;
pub use credential_service::CredentialService;
pub use file_service::FileService;
pub use forward_service::ForwardService;
pub use ftp_service::FtpService;
pub use host_service::HostService;
pub use local_file_service::{LocalFileService, unique_local_path, unique_local_path_with};
pub use monitor_service::MonitorService;
pub use s3_service::S3Service;
pub use serial_service::SerialService;
pub use setting_service::SettingService;
pub use sftp_service::{SftpService, TransferEvent, TransferHandle, TransferJob};
pub use snippet_service::SnippetService;
pub use ssh_service::SshService;
pub use telnet_service::TelnetService;
pub use trusted_host_service::TrustedHostService;
pub use webdav_service::WebdavService;
