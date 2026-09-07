//! SeaORM 实体：与 SQLite 表一一对应的数据行定义。
//!
//! 这里只声明"表长什么样"，不含任何业务规则，也不认识领域模型
//! （[`crate::host`]）—— 两边怎么对上，归服务层翻译。

pub mod credential;
pub mod host;
pub mod host_group;
pub mod setting;
pub mod snippet;
