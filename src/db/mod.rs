//! 数据基建已下沉到 `winssh-db`（见 [`winssh_db`]）：SeaORM 连接、应用级 tokio
//! runtime（[`winssh_db::run`] / [`winssh_db::app`]）、数据目录（[`winssh_db::db_dir`]）、
//! 实体定义（`entities`）与建表迁移都在那里。这里只做再导出，保持
//! `crate::db::run`、`crate::db::entities::host` 这些旧路径不变。
//!
//! 库只有一座（[`winssh_db::app`]），各服务共用同一条连接 —— 各开一条的话，
//! 退回内存库时每条连接都是独立的一座空库。

pub use winssh_db::*;
