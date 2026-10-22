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

// 面向用户的错误文案（拿不到数据目录、建表失败……）走多语言查表，`t!` 展开成
// 本 crate 的 `_rust_i18n_t!`，所以这份语言表要在本 crate 根上装一次。路径相对
// 本 crate 的 `CARGO_MANIFEST_DIR`（`crates/winssh-db/`），`../../locales` 指回仓库
// 根那一份 `locales/`，与根包共用同一批 yml；`set_locale` 是全局状态，一起切。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

/// 建表语句。`CREATE TABLE IF NOT EXISTS` 是幂等的，每次启动都跑一遍；
/// 老用户的库文件已存在时原样保留，新字段通过 [`MIGRATIONS`] 补齐。
const SCHEMA: [&str; 13] = [
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
        kind         TEXT    NOT NULL DEFAULT 'ssh',
        auth_method  TEXT    NOT NULL DEFAULT 'password',
        password     TEXT    NOT NULL DEFAULT '',
        key_content  TEXT    NOT NULL DEFAULT '',
        key_passphrase TEXT  NOT NULL DEFAULT '',
        jump_host_id TEXT,
        monitor_interval_secs INTEGER NOT NULL DEFAULT 3,
        status_interval_secs  INTEGER NOT NULL DEFAULT 8,
        forwards     TEXT    NOT NULL DEFAULT '',
        tags         TEXT    NOT NULL DEFAULT '',
        proxy        TEXT    NOT NULL DEFAULT '',
        vnc_settings TEXT    NOT NULL DEFAULT '',
        rdp_settings TEXT    NOT NULL DEFAULT '',
        serial_settings TEXT NOT NULL DEFAULT '',
        easytier_settings TEXT NOT NULL DEFAULT '',
        s3_settings TEXT NOT NULL DEFAULT '',
        ftp_settings TEXT NOT NULL DEFAULT '',
        webdav_settings TEXT NOT NULL DEFAULT '',
        shell_integration INTEGER NOT NULL DEFAULT 0,
        favorites    TEXT    NOT NULL DEFAULT '',
        sftp_settings TEXT   NOT NULL DEFAULT '',
        terminal_scheme TEXT NOT NULL DEFAULT ''
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
    // 智能助手的对话。一段对话一行；标题取自第一条提问，列表按最近更新排。
    "CREATE TABLE IF NOT EXISTS ai_conversations (
        id           TEXT    PRIMARY KEY,
        title        TEXT    NOT NULL,
        host_id      TEXT    NOT NULL DEFAULT '',
        model_id     TEXT    NOT NULL DEFAULT '',
        thinking_effort TEXT NOT NULL DEFAULT '',
        created_at   TEXT    NOT NULL DEFAULT '',
        updated_at   TEXT    NOT NULL DEFAULT ''
    )",
    // 对话里的每一条消息。`kind` 分三档：`text` 是一段话，`reasoning` 是模型
    // 开口之前的那段思考（都只用 content 一列），`command` 是一次服务器命令的
    // 执行卡片（command / output / session / status 四列才有值）。
    // `tokens_per_sec` 只有模型的话才有：那一轮每秒吐了多少 token。
    // 删一段对话连带删光它的消息，不留孤儿。
    "CREATE TABLE IF NOT EXISTS ai_messages (
        id              TEXT    PRIMARY KEY,
        conversation_id TEXT    NOT NULL REFERENCES ai_conversations(id) ON DELETE CASCADE,
        role            TEXT    NOT NULL DEFAULT 'assistant',
        kind            TEXT    NOT NULL DEFAULT 'text',
        content         TEXT    NOT NULL DEFAULT '',
        command         TEXT    NOT NULL DEFAULT '',
        output          TEXT    NOT NULL DEFAULT '',
        session         TEXT    NOT NULL DEFAULT '',
        status          TEXT    NOT NULL DEFAULT '',
        created_at      TEXT    NOT NULL DEFAULT '',
        tokens_per_sec  REAL,
        model           TEXT    NOT NULL DEFAULT ''
    )",
    // 消息上贴着的图（用户发的截图）。挂在消息上而不是对话上：一张图属于那一次
    // 提问。删对话 → 删消息 → 删图，两层级联，不留孤儿。字节是规整过的 PNG /
    // JPEG（长边不过 1568），一张几百 KB，所以单开一张表 —— 列对话、翻消息
    // 这些查询都不必把图的字节一起读出来。这张表出现之前没有图可存，老库在
    // 这里补出一张空表就是全部的迁移。
    "CREATE TABLE IF NOT EXISTS ai_images (
        id          TEXT    PRIMARY KEY,
        message_id  TEXT    NOT NULL REFERENCES ai_messages(id) ON DELETE CASCADE,
        position    INTEGER NOT NULL DEFAULT 0,
        media_type  TEXT    NOT NULL DEFAULT 'image/png',
        width       INTEGER NOT NULL DEFAULT 0,
        height      INTEGER NOT NULL DEFAULT 0,
        data        BLOB    NOT NULL
    )",
    // 读一段对话的图、删一条消息时的级联，都按 `message_id` 走。
    "CREATE INDEX IF NOT EXISTS idx_ai_images_message
        ON ai_images (message_id, position)",
    // 命令历史。和上面几张表相反，这一张**是**挂在主机上的：一条历史的意思是
    // "在这台机器上跑过这条命令"，换一台机器就不是同一件事了。所以外键指向
    // `hosts`，删掉一台主机，它的历史跟着一起走（见 `crate::command_history`）。
    //
    // 字段只有五个，每一个都得自己挣来：`exit_code` 可空（`133;D` 不带退出码
    // 时就是不知道，写 0 是句假话），`created_at` 与别处同格式，字典序即时间序。
    // 会话 id、耗时、工作目录、输出内容都**没有** —— 历史跟着主机走，不跟会话走；
    // 剩下那几样现在没人读，真要加时 `MIGRATIONS` 里一条 `ADD COLUMN` 的事。
    "CREATE TABLE IF NOT EXISTS command_history (
        id         TEXT    PRIMARY KEY,
        host_id    TEXT    NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
        command    TEXT    NOT NULL,
        exit_code  INTEGER,
        created_at TEXT    NOT NULL DEFAULT ''
    )",
    // 库里唯一一张会一直长的表：别的表都是用户一条条建出来的（几十行到头），
    // 这张按"敲过多少条命令"长。列表查询与删主机时的级联都按 `host_id` 走，
    // 没有这条索引两者都要扫全表 —— 而扫的是那张最大的表。
    "CREATE INDEX IF NOT EXISTS idx_command_history_host
        ON command_history (host_id, created_at DESC)",
    // 应用级偏好的键值对。一张表装下所有"不属于某台主机、某条凭据"的设置
    // （现在只有全局代理，见 `crate::service::SettingService`）：这些东西各自
    // 只有一行，每加一项就开一张表的话，库里很快就是一堆单行表。
    "CREATE TABLE IF NOT EXISTS settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL DEFAULT ''
    )",
    // SSH 主机信任记录（known_hosts 的应用内等价物）。
    // hostname + port 联合唯一：同一台服务器不同端口算两条独立记录。
    // 与 hosts 表无外键：信任按地址认定，与保存的主机配置无关。
    "CREATE TABLE IF NOT EXISTS trusted_hosts (
        id           TEXT    PRIMARY KEY,
        hostname     TEXT    NOT NULL,
        port         INTEGER NOT NULL DEFAULT 22,
        key_type     TEXT    NOT NULL DEFAULT '',
        fingerprint  TEXT    NOT NULL DEFAULT '',
        created_at   TEXT    NOT NULL DEFAULT '',
        last_seen    TEXT    NOT NULL DEFAULT '',
        UNIQUE (hostname, port)
    )",
    // Clash.Meta（mihomo）订阅（见 `crate::clash`）。一条记录 = 一份订阅的
    // 名字、来路（订阅地址，手工粘贴的为空）与**整份正文**。
    //
    // 正文整份存进库、而不是只存地址、启动时现拉，有两个理由：一是订阅站
    // 挂了或者换了地址，手里这份还能用；二是备份同步的是整座库（见
    // `crate::service::BackupService` 的快照），正文在库里，换台机器恢复出来
    // 就是**同一份**节点，而不是一份要靠重新联网才凑得出的配置。
    //
    // 套餐用量与到期是机场回在响应头里的（`subscription-userinfo`），存进库
    // 里让列表卡片不用每次先联网才知道流量还剩多少。`None`（NULL）是机场没
    // 回这个数 —— 那三列不是"0"，是"不知道"。
    //
    // 与 hosts 无外键：订阅是全局的，不属于哪台主机。
    "CREATE TABLE IF NOT EXISTS clash_subscriptions (
        id             TEXT    PRIMARY KEY,
        name           TEXT    NOT NULL,
        source_url     TEXT    NOT NULL DEFAULT '',
        config_yaml    TEXT    NOT NULL DEFAULT '',
        tag            TEXT    NOT NULL DEFAULT '',
        color          TEXT    NOT NULL DEFAULT '',
        upload_bytes   INTEGER,
        download_bytes INTEGER,
        total_bytes    INTEGER,
        expire_date    TEXT,
        created_at     TEXT    NOT NULL DEFAULT '',
        updated_at     TEXT    NOT NULL DEFAULT ''
    )",
];

/// 针对已存在的库文件的增量迁移。每条都是 `ADD COLUMN`，已存在时 SQLite
/// 会报 `duplicate column name`，那是幂等的成功，不算错误。
const MIGRATIONS: [&str; 36] = [
    // 连接类型（见 [`crate::host::HostKind`]）。老库补出来的是 `ssh` ——
    // 这一列出现之前存的每一台主机都是 SSH，出厂值不改变任何人的行为。
    "ALTER TABLE hosts ADD COLUMN kind TEXT NOT NULL DEFAULT 'ssh'",
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
    // VNC 画质配置（见 [`crate::vnc::VncSettings`]）。空串就是出厂档 ——
    // 真彩、自动编码、30fps、共享、3.8，这一列出现之前根本没有"画质"可言，
    // 老库补出来不改变任何一台主机现在的连法。
    "ALTER TABLE hosts ADD COLUMN vnc_settings TEXT NOT NULL DEFAULT ''",
    // 连上之后要不要往对端 shell 里装 OSC 133 标记（见
    // `crate::terminal::shell_integration`）。老库补出来的是 0，也就是"不装"
    // —— 往用户的服务器上打字这件事只能由他自己开，绝不能因为升级了一版就
    // 默默开始做。
    "ALTER TABLE hosts ADD COLUMN shell_integration INTEGER NOT NULL DEFAULT 0",
    // RDP 连接配置（见 [`crate::rdp::RdpSettings`]）。空串就是出厂档 ——
    // TLS+NLA 自动协商、真彩、平衡性能档、剪贴板开、压缩开，这一列出现之前
    // 根本没有 RDP 主机，老库补出来不影响任何一台主机现在的连法。
    "ALTER TABLE hosts ADD COLUMN rdp_settings TEXT NOT NULL DEFAULT ''",
    // 串口参数（见 [`crate::serial::SerialSettings`]）。空串就是出厂档 ——
    // 115200 8N1、无流控、DTR/RTS 不动、不回显，这一列出现之前根本没有
    // 串口主机，老库补出来不影响任何一台主机现在的连法。
    "ALTER TABLE hosts ADD COLUMN serial_settings TEXT NOT NULL DEFAULT ''",
    // 组网配置（见 [`crate::easytier::EasyTierSettings`]）。空串就是出厂档 ——
    // 用户态 SOCKS5、DHCP、不监听、加密开，这一列出现之前根本没有组网主机，
    // 老库补出来不影响任何一台主机现在的连法。
    "ALTER TABLE hosts ADD COLUMN easytier_settings TEXT NOT NULL DEFAULT ''",
    // S3 连接配置（见 [`crate::s3::S3Settings`]）。空串就是出厂档 ——
    // Bucket 与 Region 都空着，这一列出现之前根本没有 S3 主机，
    // 老库补出来不影响任何一台主机现在的连法。
    "ALTER TABLE hosts ADD COLUMN s3_settings TEXT NOT NULL DEFAULT ''",
    // FTP 加密方式（见 [`crate::ftp::FtpSettings`]）。空串就是出厂档"明文"——
    // 这一列出现之前每台 FTP 主机走的都是明文，老库补出来不改变任何人的连法。
    "ALTER TABLE hosts ADD COLUMN ftp_settings TEXT NOT NULL DEFAULT ''",
    // WebDAV 连接配置（见 [`crate::webdav::WebdavSettings`]）。空串就是出厂档 ——
    // 自动认证、校验证书、根目录空；这一列出现之前根本没有 WebDAV 主机，
    // 老库补出来不影响任何一台主机现在的连法。
    "ALTER TABLE hosts ADD COLUMN webdav_settings TEXT NOT NULL DEFAULT ''",
    // ── Clash 订阅卡片化（label + 流量用量 / 到期日） ──
    // label 与颜色是老库没有的，补出来是空串 —— 一份老订阅升级之后回到"未
    // 分组、本色"的样子，与新建时的出厂态一致。
    "ALTER TABLE clash_subscriptions ADD COLUMN tag TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE clash_subscriptions ADD COLUMN color TEXT NOT NULL DEFAULT ''",
    // 用量与到期是机场回的（`subscription-userinfo`），补出来是 NULL ——
    // "还不知道那一行"而不是 0：0 会骗人说"没用过流量"，NULL 才说"没读到"。
    "ALTER TABLE clash_subscriptions ADD COLUMN upload_bytes INTEGER",
    "ALTER TABLE clash_subscriptions ADD COLUMN download_bytes INTEGER",
    "ALTER TABLE clash_subscriptions ADD COLUMN total_bytes INTEGER",
    "ALTER TABLE clash_subscriptions ADD COLUMN expire_date TEXT",
    // ── Clash 订阅表单对齐原型（拉取 UA / 自动更新频率 / 备注） ──
    // 老库补出来都是"没有"：UA 空着走协议层那一份默认，频率 0 是手动更新，
    // 备注空串。
    "ALTER TABLE clash_subscriptions ADD COLUMN user_agent TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE clash_subscriptions ADD COLUMN auto_update_hours INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE clash_subscriptions ADD COLUMN notes TEXT NOT NULL DEFAULT ''",
    // ── 智能助手：模型每条回复的输出速度 ──
    // 可空：这一列出现之前的回复没量过，用户自己的话本来就没有这回事。
    "ALTER TABLE ai_messages ADD COLUMN tokens_per_sec REAL",
    // ── SFTP 面板收藏夹 ──
    // 老库补出来是空串：那一列出现之前没有任何主机有夹子里有东西，与出厂态一致。
    "ALTER TABLE hosts ADD COLUMN favorites TEXT NOT NULL DEFAULT ''",
    // ── SFTP 提权（sudo / 自定义服务端命令） ──
    // 空串就是出厂档"标准子系统"—— 这一列出现之前每台主机开 SFTP 走的都是它，
    // 老库补出来不改变任何人的连法，更不会在升级之后悄悄开始 sudo。
    "ALTER TABLE hosts ADD COLUMN sftp_settings TEXT NOT NULL DEFAULT ''",
    // ── 智能助手：对话绑定主机 ──
    // 每段对话记下它发起时所处标签页的主机 id（见 `crate::assistant::Conversation`），
    // 切换主机标签时助手只加载这台主机的对话。老库补出来是空串 = 旧对话没有归属，
    // 不会在任何主机的对话列表里冒出来，不改变既有行为。
    "ALTER TABLE ai_conversations ADD COLUMN host_id TEXT NOT NULL DEFAULT ''",
    // ── 智能助手：多模型 ──
    // 每段对话记下自己用的模型（配置里的模型 id）与思考强度，按对话各记各的。
    // 老库补出来是空串 = 没挑过，用新对话的默认模型，和升级前的行为一致。
    "ALTER TABLE ai_conversations ADD COLUMN model_id TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE ai_conversations ADD COLUMN thinking_effort TEXT NOT NULL DEFAULT ''",
    // 每条回复是哪个模型说的（模型名），写在回复脚注里。老回复是空串，不写。
    "ALTER TABLE ai_messages ADD COLUMN model TEXT NOT NULL DEFAULT ''",
    // ── 每主机终端配色 ──
    // 方案 id 一个词，空串是出厂档"跟随全局"（设置里挑的那一套）—— 这一列
    // 出现之前每台主机的终端都跟着全局走，老库补出来不改变任何人的观感。
    "ALTER TABLE hosts ADD COLUMN terminal_scheme TEXT NOT NULL DEFAULT ''",
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

/// 用户数据目录（winssh-pro）。
pub fn db_dir() -> Result<std::path::PathBuf> {
    let dir = dirs::data_dir()
        .with_context(|| t!("host_service.no_data_dir").to_string())?
        .join("winssh-pro");
    std::fs::create_dir_all(&dir)
        .with_context(|| t!("host_service.create_data_dir_failed").to_string())?;
    Ok(dir)
}

/// 数据库主文件路径（winssh-pro.db）。
pub fn db_file_path() -> Result<std::path::PathBuf> {
    Ok(db_dir()?.join("winssh-pro.db"))
}

/// 待还原的数据库暂存文件路径（winssh-pro.db.restored）。
pub fn restored_db_file_path() -> Result<std::path::PathBuf> {
    Ok(db_dir()?.join("winssh-pro.db.restored"))
}

/// 用户数据目录下的那个库文件。
fn open_on_disk() -> Result<DatabaseConnection> {
    let dir = db_dir()?;
    let target = dir.join("winssh-pro.db");
    let restored = dir.join("winssh-pro.db.restored");

    // 若存在待还原的备份文件，在建立数据库连接前将其安全置换
    if restored.exists() {
        if target.exists() {
            let backup_prev = dir.join(format!(
                "winssh-pro.db.pre_restore_{}",
                chrono::Local::now().format("%Y%m%d_%H%M%S")
            ));
            _ = std::fs::rename(&target, &backup_prev);
        }
        let wal = dir.join("winssh-pro.db-wal");
        let shm = dir.join("winssh-pro.db-shm");
        if wal.exists() {
            _ = std::fs::remove_file(wal);
        }
        if shm.exists() {
            _ = std::fs::remove_file(shm);
        }
        if let Err(e) = std::fs::rename(&restored, &target) {
            log::error!("应用备份还原文件失败：{e:#}");
        } else {
            log::info!("已从备份成功还原数据库并生效");
        }
    }

    connect_blocking(Some(&target))
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
