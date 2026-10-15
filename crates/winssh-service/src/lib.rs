//! 服务层：界面拿数据的唯一入口。
//!
//! 界面只认领域模型（[`crate::host`]）和这里的异步方法，不知道 SeaORM
//! 与 SQL 的存在；业务规则（id 分配、副本命名、删分组的前置条件……）
//! 也都收在这里，数据仓只提供一条条数据操作。

// 门面再导出：把已下沉的各 crate / 领域子模块在本 crate 根上起个别名，
// 这样服务层各文件里的 `crate::host::Host`、`crate::db::run`、`crate::ssh::…`
// 等旧路径无需逐文件改写就能解析到新的 crate。（用 `pub use`：既作 crate 内路径
// 别名，又不触发 unused_imports。）
pub use winssh_domain::{
    command_history, credential, forward, host, session, snippet, tray, trusted_host,
};
// `crate::vnc` / `crate::rdp` 在服务层只用作连接设置的编解码（都在领域的 settings 里）。
pub use winssh_domain::settings::{rdp, vnc};
pub use winssh_backup as backup;
pub use winssh_clash as clash;
pub use winssh_db as db;
pub use winssh_desktop as desktop;
pub use winssh_easytier as easytier;
pub use winssh_ftp as ftp;
pub use winssh_localfs as localfs;
pub use winssh_monitor as monitor;
pub use winssh_netstats as netstats;
pub use winssh_proxy as proxy;
pub use winssh_repository as repository;
pub use winssh_s3 as s3;
pub use winssh_serial as serial;
pub use winssh_sftp as sftp;
pub use winssh_ssh as ssh;
pub use winssh_telnet as telnet;
pub use winssh_terminal as terminal;
pub use winssh_webdav as webdav;
pub use winssh_zmodem as zmodem;
use winssh_assistant as assistant;

// 面向用户的文案走多语言查表，`t!` 展开成本 crate 的 `_rust_i18n_t!`；
// `../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR 指回仓库根。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

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
