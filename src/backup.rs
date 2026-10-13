//! 数据备份与恢复：WebDAV 与 S3 兼容协议的领域模型、加密机制与预设。
//!
//! 核心能力：
//! - 协议支持：WebDAV（基于 Apache OpenDAL）、S3 兼容协议（基于 rust-s3）
//! - 常见 S3 云厂商预设：AWS S3、阿里云 OSS、腾讯云 COS、七牛云 Kodo、MinIO/自定义
//! - 加密机制：支持明文裸备份与 AES-256-GCM 密码加密备份（PBKDF2-SHA256 密钥派生）
//! - 副本保留策略与备份频率设置

use std::sync::{OnceLock, RwLock};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Context as _, Result, anyhow, bail};
use pbkdf2::pbkdf2_hmac;
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// 备份文件魔数头：用于校验与识别备份格式。
pub const BACKUP_MAGIC: &[u8; 6] = b"WSPBAK";
/// 备份格式版本。
pub const BACKUP_VERSION: u8 = 1;
/// 未加密标志（裸备份）。
pub const FLAG_RAW: u8 = 0;
/// AES-256-GCM 加密标志。
pub const FLAG_AES256_GCM: u8 = 1;

/// PBKDF2 迭代次数。
const PBKDF2_ROUNDS: u32 = 100_000;
/// 盐长度（16 字节）。
const SALT_LEN: usize = 16;
/// Nonce 长度（12 字节，96 位）。
const NONCE_LEN: usize = 12;

/// 备份协议类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackupProtocol {
    #[default]
    Webdav,
    S3,
}

impl BackupProtocol {
    pub const ALL: [Self; 2] = [Self::Webdav, Self::S3];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Webdav => "webdav",
            Self::S3 => "s3",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Webdav => "WebDAV",
            Self::S3 => "S3 兼容协议",
        }
    }

    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "s3" => Self::S3,
            _ => Self::Webdav,
        }
    }
}

/// WebDAV 配置。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebdavConfig {
    /// 服务器完整地址，如 `https://dav.jianguoyun.com/dav/`。
    #[serde(default)]
    pub endpoint: String,
    /// 备份根目录，如 `/winssh_backups`。
    #[serde(default = "default_backup_dir")]
    pub root: String,
    /// 认证用户名。
    #[serde(default)]
    pub username: String,
    /// 认证密码或应用授权码。
    #[serde(default)]
    pub password: String,
}

impl Default for WebdavConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            root: default_backup_dir(),
            username: String::new(),
            password: String::new(),
        }
    }
}

fn default_backup_dir() -> String {
    "/winssh_backups".to_string()
}

/// S3 服务商预设定义。端点 / Region / 寻址风格从 [`crate::s3::presets`] 那份
/// 共享事实派生（两处不再各写一份），这里只补界面要的名字。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct S3Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub default_endpoint: &'static str,
    pub default_region: &'static str,
    pub path_style: bool,
}

use crate::s3::presets as s3_presets;
use crate::s3::S3Addressing;

const fn preset_of(
    shared: &s3_presets::S3Preset,
    name: &'static str,
) -> S3Preset {
    S3Preset {
        id: shared.id,
        name,
        default_endpoint: shared.default_endpoint,
        default_region: shared.default_region,
        path_style: !matches!(shared.addressing, S3Addressing::VirtualHost),
    }
}

/// 内置常见 S3 云厂商预设列表。id 与 [`crate::s3::presets`] 对齐 ——
/// 备份页沿用的 "custom" 在主机表单那边拆成了 MinIO 与自定义两档。
pub const S3_PRESETS: &[S3Preset] = &[
    preset_of(&s3_presets::AWS, "Amazon S3"),
    preset_of(&s3_presets::ALIYUN, "阿里云 OSS"),
    preset_of(&s3_presets::TENCENT, "腾讯云 COS"),
    preset_of(&s3_presets::QINIU, "七牛云 Kodo"),
    preset_of(&s3_presets::MINIO, "自定义 / MinIO"),
];

pub fn find_s3_preset(id: &str) -> Option<&'static S3Preset> {
    S3_PRESETS.iter().find(|preset| preset.id == id)
}

/// S3 兼容协议配置。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Config {
    /// 预设标识符（"aws" | "aliyun" | "tencent" | "qiniu" | "custom"）。
    #[serde(default = "default_preset")]
    pub preset: String,
    /// 节点服务地址（Endpoint）。
    #[serde(default)]
    pub endpoint: String,
    /// 存储区域（Region）。
    #[serde(default)]
    pub region: String,
    /// 存储桶名称（Bucket）。
    #[serde(default)]
    pub bucket: String,
    /// 存储根目录 / 前缀路径。
    #[serde(default = "default_s3_root")]
    pub root: String,
    /// 访问密钥 Access Key ID。
    #[serde(default)]
    pub access_key_id: String,
    /// 访问私钥 Secret Access Key。
    #[serde(default)]
    pub secret_access_key: String,
    /// 是否使用路径寻址模式（Path-Style，MinIO/Ceph 常需要开启）。
    #[serde(default)]
    pub path_style: bool,
}

fn default_preset() -> String {
    "aws".to_string()
}

fn default_s3_root() -> String {
    "winssh_backups".to_string()
}

impl Default for S3Config {
    fn default() -> Self {
        let aws = &S3_PRESETS[0];
        Self {
            preset: aws.id.to_string(),
            endpoint: aws.default_endpoint.to_string(),
            region: aws.default_region.to_string(),
            bucket: String::new(),
            root: default_s3_root(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
            path_style: aws.path_style,
        }
    }
}

/// 完整的数据备份同步配置。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupConfig {
    /// 是否开启定时自动备份。
    #[serde(default)]
    pub enabled: bool,
    /// 当前选用的协议。
    #[serde(default)]
    pub protocol: BackupProtocol,
    /// 备份频率（分钟）。默认 60 分钟。
    #[serde(default = "default_interval_mins")]
    pub interval_mins: u32,
    /// 保留副本数。默认 10 份。
    #[serde(default = "default_retention_count")]
    pub retention_count: u32,
    /// 是否开启备份加密。
    #[serde(default)]
    pub encrypt: bool,
    /// AES 加密密码。若为空或未开启加密，则直接进行裸备份。
    #[serde(default)]
    pub password: String,
    /// WebDAV 配置。
    #[serde(default)]
    pub webdav: WebdavConfig,
    /// S3 配置。
    #[serde(default)]
    pub s3: S3Config,
    /// 上次成功备份的时间戳记录（ISO 格式或人类可读）。
    #[serde(default)]
    pub last_backup_at: Option<String>,
    /// 上次备份执行结果状态描述。
    #[serde(default)]
    pub last_backup_status: Option<String>,
}

fn default_interval_mins() -> u32 {
    60
}

fn default_retention_count() -> u32 {
    10
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            protocol: BackupProtocol::default(),
            interval_mins: default_interval_mins(),
            retention_count: default_retention_count(),
            encrypt: false,
            password: String::new(),
            webdav: WebdavConfig::default(),
            s3: S3Config::default(),
            last_backup_at: None,
            last_backup_status: None,
        }
    }
}

/// 远端备份文件元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupRemoteFile {
    /// 文件名，如 `winssh_backup_20260921_120000.wspbak`。
    pub name: String,
    /// 文件大小（字节）。
    pub size: u64,
    /// 修改时间或创建时间描述。
    pub updated_at: Option<String>,
}

// ── 全局缓存 ─────────────────────────────────────────────────────────────

static GLOBAL_BACKUP_CONFIG: OnceLock<RwLock<BackupConfig>> = OnceLock::new();

fn global_holder() -> &'static RwLock<BackupConfig> {
    GLOBAL_BACKUP_CONFIG.get_or_init(|| RwLock::new(BackupConfig::default()))
}

/// 获取全局当前生效的备份配置克隆。
pub fn global() -> BackupConfig {
    global_holder().read().unwrap().clone()
}

/// 更新全局备份配置缓存。
pub fn set_global(config: BackupConfig) {
    *global_holder().write().unwrap() = config;
}

/// 测试用：把"这条测试要动备份配置"串起来。
///
/// `cargo test` 是并行的，而 [`GLOBAL_BACKUP_CONFIG`] 是整个进程共用的一份
/// —— 不串起来的话，一条测试设的值会落到另一条的断言上，红得毫无规律。凡是
/// 调 [`set_global`] 的测试都先拿这把锁，返回的守卫掉地时把全局放回出厂档，
/// panic 了也一样。
#[cfg(test)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global(BackupConfig::default());
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

// ── 加密与解密实现 ─────────────────────────────────────────────────────────

/// 将数据库二进制快照打包为备份数据包。
///
/// 若 `password` 存在且非空，使用 AES-256-GCM 加密；否则输出带有 Raw 标记的裸备份数据包。
pub fn encrypt_backup(raw_db: &[u8], password: Option<&str>) -> Result<Vec<u8>> {
    let password = password.map(str::trim).unwrap_or("");
    if password.is_empty() {
        // 裸备份：魔数(6) + 版本(1) + 标志0(1) + 数据
        let mut out = Vec::with_capacity(8 + raw_db.len());
        out.extend_from_slice(BACKUP_MAGIC);
        out.push(BACKUP_VERSION);
        out.push(FLAG_RAW);
        out.extend_from_slice(raw_db);
        return Ok(out);
    }

    // AES-256-GCM 加密
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    let mut rng = rand::rng();
    rng.fill(&mut salt);
    rng.fill(&mut nonce_bytes);

    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, PBKDF2_ROUNDS, &mut key);

    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow!("初始化 AES 加密器失败: {e}"))?;
    let nonce = Nonce::from(nonce_bytes);
    let ciphertext = cipher
        .encrypt(&nonce, raw_db)
        .map_err(|e| anyhow!("AES 加密数据失败: {e}"))?;

    let mut out = Vec::with_capacity(8 + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(BACKUP_MAGIC);
    out.push(BACKUP_VERSION);
    out.push(FLAG_AES256_GCM);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// 解析并解密备份数据包，还原出原始 SQLite 数据库字节流。
///
/// 兼容直接上传的原始 SQLite 数据库文件（魔数以 `SQLite format 3\0` 开头）。
pub fn decrypt_backup(payload: &[u8], password: Option<&str>) -> Result<Vec<u8>> {
    // 兼容未经封装的原生 SQLite 库文件
    if payload.starts_with(b"SQLite format 3\0") {
        return Ok(payload.to_vec());
    }

    if payload.len() < 8 || &payload[0..6] != BACKUP_MAGIC {
        bail!("无效的备份文件格式：缺少 WSPBAK 签名");
    }

    let version = payload[6];
    if version != BACKUP_VERSION {
        bail!("不受支持的备份版本号：{version}");
    }

    let flag = payload[7];
    match flag {
        FLAG_RAW => {
            // 裸备份，直接提取数据
            Ok(payload[8..].to_vec())
        }
        FLAG_AES256_GCM => {
            let password = password.map(str::trim).unwrap_or("");
            if password.is_empty() {
                bail!("此备份已通过 AES 加密，请输入解密密码");
            }

            let min_len = 8 + SALT_LEN + NONCE_LEN + 16; // 至少包含 16 字节 GCM tag
            if payload.len() < min_len {
                bail!("备份文件损坏：数据长度过短");
            }

            let salt = &payload[8..8 + SALT_LEN];
            let nonce_bytes: [u8; NONCE_LEN] = payload[8 + SALT_LEN..8 + SALT_LEN + NONCE_LEN]
                .try_into()
                .map_err(|_| anyhow!("Nonce 长度错误"))?;
            let ciphertext = &payload[8 + SALT_LEN + NONCE_LEN..];

            let mut key = [0u8; 32];
            pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, PBKDF2_ROUNDS, &mut key);

            let cipher = Aes256Gcm::new_from_slice(&key)
                .map_err(|e| anyhow!("初始化 AES 解密器失败: {e}"))?;
            let nonce = Nonce::from(nonce_bytes);
            let plaintext = cipher
                .decrypt(&nonce, ciphertext)
                .map_err(|_| anyhow!("解密失败：密码错误或备份文件已损坏"))?;

            Ok(plaintext)
        }
        unknown => bail!("未知的加密类型代码：{unknown}"),
    }
}

// ── 远端存储接入 ──────────────────────────────────────────────────────────

use ::s3::Bucket;
use ::s3::error::S3Error;
use opendal::Operator;
use opendal::services::Webdav;

/// 备份落点。WebDAV 走 OpenDAL，S3 走 rust-s3（与主机的 S3 文件面板同一份
/// 接线，见 [`crate::s3::S3ConnParams`]）。备份服务只用得着列、写、读、删
/// 四个动作，两边在这里折成同一套；副本一律平铺在 root 下。
///
/// 构造只装参数、不发请求。四个动作都要在数据层 runtime 上 await —— 两边
/// 底层都是 reqwest + tokio，在 gpui executor 上 await 会因缺 tokio reactor
/// 直接 panic。
pub enum BackupStore {
    Webdav(Operator),
    S3 {
        bucket: Box<Bucket>,
        /// root 前缀，不带首尾 `/`；空串即桶根。
        root: String,
    },
}

impl BackupStore {
    /// 根据配置装出备份落点。
    pub fn open(config: &BackupConfig) -> Result<Self> {
        match config.protocol {
            BackupProtocol::Webdav => {
                let mut endpoint = config.webdav.endpoint.trim().to_string();
                if endpoint.is_empty() {
                    bail!("WebDAV 服务器地址不能为空");
                }
                if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                    endpoint = format!("https://{endpoint}");
                }
                if !endpoint.ends_with('/') {
                    endpoint.push('/');
                }

                let mut builder = Webdav::default().endpoint(&endpoint);

                let root = config.webdav.root.trim();
                if !root.is_empty() {
                    builder = builder.root(root);
                }

                let user = config.webdav.username.trim();
                if !user.is_empty() {
                    builder = builder.username(user);
                }
                let pass = config.webdav.password.trim();
                if !pass.is_empty() {
                    builder = builder.password(pass);
                }

                let op = Operator::new(builder).context("构建 WebDAV 存储客户端失败")?;
                Ok(Self::Webdav(op))
            }
            BackupProtocol::S3 => {
                let bucket = config.s3.bucket.trim();
                if bucket.is_empty() {
                    bail!("S3 存储桶名称 (Bucket) 不能为空");
                }
                // Region 留空走端点推断、推不出折回 us-east-1；端点留空落到
                // AWS 的地域端点。两条兜底都在 `S3ConnParams::new` 里。
                let addressing = match config.s3.path_style {
                    true => S3Addressing::Path,
                    false => S3Addressing::VirtualHost,
                };
                let params = crate::s3::S3ConnParams::new(
                    &config.s3.endpoint,
                    &config.s3.access_key_id,
                    &config.s3.secret_access_key,
                    &config.s3.region,
                    addressing,
                );
                let bucket = params.bucket(bucket).context("构建 S3 存储客户端失败")?;
                Ok(Self::S3 {
                    bucket,
                    root: config.s3.root.trim().trim_matches('/').to_string(),
                })
            }
        }
    }

    /// 列出 root 下直接的那一层文件：`(文件名, 大小)`。
    pub async fn list(&self) -> Result<Vec<(String, u64)>> {
        match self {
            Self::Webdav(op) => Ok(op
                .list("")
                .await?
                .into_iter()
                .map(|entry| (entry.name().to_string(), entry.metadata().content_length()))
                .collect()),
            Self::S3 { bucket, root } => {
                let prefix = s3_key(root, "");
                let pages = bucket
                    .list(prefix.clone(), Some("/".to_string()))
                    .await
                    .map_err(s3_error)?;
                Ok(pages
                    .into_iter()
                    .flat_map(|page| page.contents)
                    .filter_map(|object| {
                        let name = object.key.strip_prefix(&prefix)?;
                        (!name.is_empty()).then(|| (name.to_string(), object.size))
                    })
                    .collect())
            }
        }
    }

    /// 写一个副本（同名覆盖）。
    pub async fn write(&self, name: &str, payload: Vec<u8>) -> Result<()> {
        match self {
            Self::Webdav(op) => op.write(name, payload).await.map(drop)?,
            Self::S3 { bucket, root } => s3_transfer(bucket)
                .put_object(s3_key(root, name), &payload)
                .await
                .map(drop)
                .map_err(s3_error)?,
        }
        Ok(())
    }

    /// 读回一个副本的全部字节。
    pub async fn read(&self, name: &str) -> Result<Vec<u8>> {
        match self {
            Self::Webdav(op) => Ok(op.read(name).await?.to_vec()),
            Self::S3 { bucket, root } => Ok(s3_transfer(bucket)
                .get_object(s3_key(root, name))
                .await
                .map_err(s3_error)?
                .to_vec()),
        }
    }

    /// 删一个副本。
    pub async fn delete(&self, name: &str) -> Result<()> {
        match self {
            Self::Webdav(op) => op.delete(name).await?,
            Self::S3 { bucket, root } => bucket
                .delete_object(s3_key(root, name))
                .await
                .map(drop)
                .map_err(s3_error)?,
        }
        Ok(())
    }
}

/// 副本在桶里的 key：`{root}/{name}`，root 为空就是桶根。`name` 传空串得到的
/// 正是列举 root 那一层用的前缀（`{root}/` 或空）。
fn s3_key(root: &str, name: &str) -> String {
    match root.is_empty() {
        true => name.to_string(),
        false => format!("{root}/{name}"),
    }
}

/// 上传 / 下载副本用的那一只：去掉 rust-s3 默认的 60 秒整请求超时 —— 它连
/// 正文一起算，几十 MB 的快照在慢上行链路上一分钟传不完。克隆共用同一只
/// reqwest 客户端。
fn s3_transfer(bucket: &Bucket) -> Bucket {
    let mut bucket = bucket.clone();
    bucket.set_request_timeout(None);
    bucket
}

/// rust-s3 的错误说成一行人话（对端拒绝时带着 S3 的错误码与原话）。
fn s3_error(error: S3Error) -> anyhow::Error {
    anyhow!(crate::s3::describe(&error))
}

#[cfg(test)]
#[path = "tests/backup.rs"]
mod tests;
