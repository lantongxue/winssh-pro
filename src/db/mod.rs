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
use rust_i18n::t;
use sea_orm::sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sea_orm::{ConnectionTrait, DatabaseConnection, SqlxSqliteConnector};
use tokio::runtime::Runtime;

/// 建表语句。`CREATE TABLE IF NOT EXISTS` 是幂等的，每次启动都跑一遍；
/// 老用户的库文件已存在时原样保留，新字段通过 [`MIGRATIONS`] 补齐。
const SCHEMA: [&str; 5] = [
    "CREATE TABLE IF NOT EXISTS host_groups (
        id         INTEGER PRIMARY KEY,
        name       TEXT    NOT NULL UNIQUE,
        sort_order INTEGER NOT NULL DEFAULT 0
    )",
    "CREATE TABLE IF NOT EXISTS hosts (
        id           TEXT    PRIMARY KEY,
        group_id     INTEGER NOT NULL REFERENCES host_groups(id) ON DELETE CASCADE,
        name         TEXT    NOT NULL,
        user         TEXT    NOT NULL,
        hostname     TEXT    NOT NULL,
        port         INTEGER NOT NULL DEFAULT 22,
        platform     TEXT    NOT NULL DEFAULT '',
        sort_order   INTEGER NOT NULL DEFAULT 0,
        auth_method  TEXT    NOT NULL DEFAULT 'password',
        password     TEXT    NOT NULL DEFAULT '',
        key_content  TEXT    NOT NULL DEFAULT '',
        key_passphrase TEXT  NOT NULL DEFAULT '',
        jump_host_id TEXT,
        monitor_interval_secs INTEGER NOT NULL DEFAULT 3,
        status_interval_secs  INTEGER NOT NULL DEFAULT 8,
        forwards     TEXT    NOT NULL DEFAULT '',
        tags         TEXT    NOT NULL DEFAULT '',
        proxy        TEXT    NOT NULL DEFAULT ''
    )",
    // 保险箱。和 hosts 之间没有外键：主机存的是凭据的内容本身，不是一个指向
    // 这里的 id（见 `crate::credential` 的模块文档），删一条凭据不牵动任何主机。
    "CREATE TABLE IF NOT EXISTS credentials (
        id           TEXT    PRIMARY KEY,
        name         TEXT    NOT NULL,
        kind         TEXT    NOT NULL DEFAULT 'password',
        category     TEXT    NOT NULL DEFAULT '',
        username     TEXT    NOT NULL DEFAULT '',
        password     TEXT    NOT NULL DEFAULT '',
        algorithm    TEXT    NOT NULL DEFAULT '',
        public_key   TEXT    NOT NULL DEFAULT '',
        private_key  TEXT    NOT NULL DEFAULT '',
        passphrase   TEXT    NOT NULL DEFAULT '',
        fingerprint  TEXT    NOT NULL DEFAULT '',
        token        TEXT    NOT NULL DEFAULT '',
        tags         TEXT    NOT NULL DEFAULT '',
        notes        TEXT    NOT NULL DEFAULT '',
        favorite     INTEGER NOT NULL DEFAULT 0,
        created_at   TEXT    NOT NULL DEFAULT '',
        updated_at   TEXT    NOT NULL DEFAULT ''
    )",
    // 命令片段。和 hosts 之间也没有关系：一条片段发到哪台机器上，是按下"发送"
    // 那一刻才决定的（见 `crate::snippet` 的模块文档），所以它不属于任何一台主机。
    "CREATE TABLE IF NOT EXISTS snippets (
        id           TEXT    PRIMARY KEY,
        title        TEXT    NOT NULL,
        category     TEXT    NOT NULL DEFAULT 'docker',
        command      TEXT    NOT NULL DEFAULT '',
        description  TEXT    NOT NULL DEFAULT '',
        tags         TEXT    NOT NULL DEFAULT '',
        variables    TEXT    NOT NULL DEFAULT '',
        created_at   TEXT    NOT NULL DEFAULT '',
        updated_at   TEXT    NOT NULL DEFAULT ''
    )",
    // 应用级偏好的键值对。一张表装下所有"不属于某台主机、某条凭据"的设置
    // （现在只有全局代理，见 `crate::service::SettingService`）：这些东西各自
    // 只有一行，每加一项就开一张表的话，库里很快就是一堆单行表。
    "CREATE TABLE IF NOT EXISTS settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL DEFAULT ''
    )",
];

/// 针对已存在的库文件的增量迁移。每条都是 `ADD COLUMN`，已存在时 SQLite
/// 会报 `duplicate column name`，那是幂等的成功，不算错误。
const MIGRATIONS: [&str; 10] = [
    "ALTER TABLE hosts ADD COLUMN auth_method TEXT NOT NULL DEFAULT 'password'",
    "ALTER TABLE hosts ADD COLUMN password TEXT NOT NULL DEFAULT ''",
    // 私钥本身与解开它的密码。老库补出来都是空串，也就是"这台主机还没配私钥"。
    //
    // 老库里可能还有一列 `key_path`：那是早先按路径存私钥的做法，现在换成了整份
    // 内容（见 [`crate::host::Host::key_content`]）。那一列没人再读，也不删 ——
    // SQLite 的 DROP COLUMN 要重建整张表，为一列从来没被界面写过的死数据不值当。
    "ALTER TABLE hosts ADD COLUMN key_content TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE hosts ADD COLUMN key_passphrase TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE hosts ADD COLUMN jump_host_id TEXT",
    // 默认值要和 `MonitorIntervals::default()` 对得上：老库里补出来的那一列
    // 就是新主机的出厂节奏，两边不一致的话，同一台机器在升级前后表现不同。
    "ALTER TABLE hosts ADD COLUMN monitor_interval_secs INTEGER NOT NULL DEFAULT 3",
    "ALTER TABLE hosts ADD COLUMN status_interval_secs INTEGER NOT NULL DEFAULT 8",
    // 端口转发规则（见 [`crate::forward`]）。老库补出来的是空串，
    // 读回来就是"这台主机还没有转发规则"，与新库一致。
    "ALTER TABLE hosts ADD COLUMN forwards TEXT NOT NULL DEFAULT ''",
    // 主机标签列表（JSON 字符串）。老库补出来的是空串，与新库一致。
    "ALTER TABLE hosts ADD COLUMN tags TEXT NOT NULL DEFAULT ''",
    // 代理设置（见 [`crate::proxy`]）。空串就是出厂档"跟随全局"，而全局的
    // 出厂档是不使用代理 —— 所以老库补出来的这一列不改变任何一台主机现在的
    // 连法，仍旧是直连。
    "ALTER TABLE hosts ADD COLUMN proxy TEXT NOT NULL DEFAULT ''",
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
    async move { handle.spawn(work).await.expect("数据层 runtime 意外退出") }
}

/// 同步入口：连上库并建表。只在开窗前的启动阶段调用一次，
/// 一次性阻塞换组装简单。
pub fn connect_blocking(path: Option<&Path>) -> Result<DatabaseConnection> {
    runtime().block_on(connect(path))
}

/// 应用的那一座库。
///
/// 进程里只有这一座，各个服务共用同一条连接：主机和保险箱住在同一个库文件里，
/// 各开一条连接的话，磁盘库上是白白多一把写锁，而退回内存库时更糟 ——
/// 每条连接都是**独立的一座内存库**（连接池因此压到 1，见 [`connect`]），
/// 那样主机服务和保险箱服务会各自对着一座空库说话。
pub struct AppDatabase {
    pub conn: DatabaseConnection,
    /// 库文件是否真的落在磁盘上。退回内存库时为假 —— 应用照常可用，
    /// 只是改动不留存，界面装配完之后会就这件事提醒一次。
    pub persistent: bool,
}

/// 打开（必要时创建）用户数据目录下的库，进程生命周期内只做一次。
/// 打不开就退回内存库。
///
/// 同步是因为它只在开窗前的启动阶段被第一次调用；一次性阻塞换组装简单。
pub fn app() -> &'static AppDatabase {
    static APP: OnceLock<AppDatabase> = OnceLock::new();
    APP.get_or_init(|| match open_on_disk() {
        Ok(conn) => AppDatabase {
            conn,
            persistent: true,
        },
        Err(error) => {
            log::warn!("磁盘库打不开，这次运行退回内存库：{error:#}");
            AppDatabase {
                conn: connect_blocking(None).expect("初始化内存数据库失败"),
                persistent: false,
            }
        }
    })
}

/// 用户数据目录下的那个库文件。
fn open_on_disk() -> Result<DatabaseConnection> {
    let dir = dirs::data_dir()
        .with_context(|| t!("host_service.no_data_dir").to_string())?
        .join("winssh-pro");
    std::fs::create_dir_all(&dir)
        .with_context(|| t!("host_service.create_data_dir_failed").to_string())?;
    connect_blocking(Some(&dir.join("winssh-pro.db")))
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
        .with_context(|| t!("db.open_failed").to_string())?;
    let conn = SqlxSqliteConnector::from_sqlx_sqlite_pool(pool);

    for statement in SCHEMA {
        conn.execute_unprepared(statement)
            .await
            .with_context(|| t!("db.init_table_failed").to_string())?;
    }
    for migration in MIGRATIONS {
        let result = conn.execute_unprepared(migration).await;
        if let Err(error) = result {
            let message = error.to_string();
            // 已存在的列是幂等成功，不算错；其余错误才需要冒泡。
            if !message.contains("duplicate column name") {
                return Err(error).with_context(|| t!("db.migrate_failed").to_string())?;
            }
        }
    }
    Ok(conn)
}
