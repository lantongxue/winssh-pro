//! 数据仓：把 SeaORM 的查询收拢成一组命名清楚的数据操作。
//!
//! 数据仓只有数据访问，不掺业务规则 —— 什么时候插到组尾、副本叫什么
//! 名字、删分组有什么前置条件，都归服务层；这里回答的只有"怎么把这几行
//! 数据查出来、改回去"。
//!
//! 方法都接受 `impl ConnectionTrait`：传 [`DatabaseConnection`] 是单条
//! 操作，传事务则由服务层把几条捆成一个原子操作。

pub mod assistant_repository;
pub mod command_history_repository;
pub mod credential_repository;
pub mod host_repository;
pub mod setting_repository;
pub mod snippet_repository;

pub use assistant_repository::AssistantRepository;
pub use command_history_repository::CommandHistoryRepository;
pub use credential_repository::CredentialRepository;
pub use host_repository::HostRepository;
pub use setting_repository::SettingRepository;
pub use snippet_repository::SnippetRepository;
