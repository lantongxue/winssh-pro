//! 数据基建：SeaORM 的连接、应用级 tokio runtime 与建表迁移。
//!
//! SeaORM 是异步的（sqlx + tokio），而 gpui 有自己的 executor，两者不共享
//! 异步运行时。所以这里养一个进程级的多线程 tokio runtime：数据层的异步
//! 操作都派到它上面跑（[`run`]），gpui 侧拿到的只是一段普通的 future，
//! 在哪个 executor 上 await 都行。
//!
//! 库文件落在用户数据目录 `winssh-pro/winssh-pro.db`，建表语句与旧版
//! 逐字相同，老用户的库文件直接沿用。

pub mod entities;

use std::future::Future;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context as _, Result};
use sea_orm::sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sea_orm::{ConnectionTrait, DatabaseConnection, SqlxSqliteConnector};
use tokio::runtime::Runtime;

/// 与初版逐字相同的建表语句。`CREATE TABLE IF NOT EXISTS` 是幂等的，
/// 每次启动都跑一遍；库文件已存在时原样保留。要改表结构时，往后追加
/// 迁移语句，不动已有的这几行。
const SCHEMA: [&str; 2] = [
    "CREATE TABLE IF NOT EXISTS host_groups (
        id         INTEGER PRIMARY KEY,
        name       TEXT    NOT NULL UNIQUE,
        sort_order INTEGER NOT NULL DEFAULT 0
    )",
    "CREATE TABLE IF NOT EXISTS hosts (
        id         TEXT    PRIMARY KEY,
        group_id   INTEGER NOT NULL REFERENCES host_groups(id) ON DELETE CASCADE,
        name       TEXT    NOT NULL,
        user       TEXT    NOT NULL,
        hostname   TEXT    NOT NULL,
        port       INTEGER NOT NULL DEFAULT 22,
        platform   TEXT    NOT NULL DEFAULT '',
        sort_order INTEGER NOT NULL DEFAULT 0
    )",
];

/// 数据层专用的 tokio runtime，进程生命周期内只建一次。
pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("初始化数据层 tokio runtime 失败")
    })
}

/// 把一段异步工作派到数据层 runtime 上。返回的 future 只是个取结果的
/// 信封，可以在任何 executor 上 await —— 这是 SeaORM 与 gpui 之间的界河。
pub fn run<F, T>(work: F) -> impl Future<Output = T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handle = runtime().handle().clone();
    async move {
        handle
            .spawn(work)
            .await
            .expect("数据层 runtime 意外退出")
    }
}

/// 同步入口：连上库并建表。只在开窗前的启动阶段调用一次，
/// 一次性阻塞换组装简单。
pub fn connect_blocking(path: Option<&Path>) -> Result<DatabaseConnection> {
    runtime().block_on(connect(path))
}

/// 连接 SQLite 并确保表都在。`None` 是内存库：测试与磁盘库打不开时的退路。
/// 连接池压到 1：内存库必须单连接（每个连接都是独立的一座内存库），
/// 磁盘库也顺带免掉写锁竞争 —— 本地这几十行数据，串行足矣。
pub async fn connect(path: Option<&Path>) -> Result<DatabaseConnection> {
    let options = match path {
        Some(path) => SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
        None => SqliteConnectOptions::new().filename(":memory:"),
    }
    // 删除分组时连带删掉组内主机，不留悬空引用。
    .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .context("打开 SQLite 连接失败")?;
    let conn = SqlxSqliteConnector::from_sqlx_sqlite_pool(pool);

    for statement in SCHEMA {
        conn.execute_unprepared(statement)
            .await
            .context("初始化主机配置表失败")?;
    }
    Ok(conn)
}
