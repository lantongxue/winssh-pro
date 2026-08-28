//! 数据仓：把 SeaORM 的查询收拢成一组命名清楚的数据操作。
//!
//! 数据仓只有数据访问，不掺业务规则 —— 什么时候插到组尾、副本叫什么
//! 名字、删分组有什么前置条件，都归服务层；这里回答的只有"怎么把这几行
//! 数据查出来、改回去"。
//!
//! 方法都接受 `impl ConnectionTrait`：传 [`DatabaseConnection`] 是单条
//! 操作，传事务则由服务层把几条捆成一个原子操作。

pub mod host_repository;

pub use host_repository::HostRepository;
