//! WebDAV 落点与目录条目：备份同步只用到列 / 写 / 读 / 删四个动作，
//! 主机文件面板用到的那一整组在 [`crate::service::WebdavService`]。
//!
//! 这里同时是 [`crate::backup::BackupStore`] 的 WebDAV 那一档 —— 它以前落在
//! OpenDAL 的 `Operator` 上，换成我们自己的 [`WebdavClient`] 之后，备份与
//! 文件面板走的是同一套请求。
//!
//! **落点路径与 OpenDAL 逐字一致**：那时写的是 `endpoint` 的路径 + `root`，
//! 也就是服务器上的 `{端点路径}/{root}/{文件名}`（`https://host/dav` + 根目录
//! `/winssh_backups` → `/dav/winssh_backups/winssh_backup_….wspbak`）。换成
//! 我们这一套之后路径一个字都没变，已经传上去的备份原地还在、还认得出来。
//! 所以这里**不做** [`crate::webdav::client::home_within`] 那种"把挂载点从根
//! 目录里剥掉"的纠偏 —— 那会把落点挪到 `/winssh_backups/…` 去。

use anyhow::Result;

use super::client::WebdavClient;
use super::settings::WebdavSettings;
use winssh_sftp::path;

/// 目录列表里的一条，已经折算成面板口径（路径是面板绝对路径、名字是原字符）。
#[derive(Clone, Debug)]
pub struct WebdavEntry {
    pub name: String,
    /// 面板绝对路径。
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

/// 备份落点。端点 + 一个根目录，四个动作都相对它。
///
/// 便宜克隆（`WebdavClient` 内部是 `Arc`）。
#[derive(Clone)]
pub struct WebdavStore {
    client: WebdavClient,
    /// 落点目录的面板路径（`/winssh_backups`；根目录为空时是 `/`）。
    dir: String,
}

impl WebdavStore {
    /// 装出一个备份落点。不发请求。
    ///
    /// 认证走 [`AuthMode::Auto`](crate::webdav::AuthMode) 起步的 Basic：备份这一档
    /// 是后台定时跑的，多问一次服务器换 Digest 不值当，也同 OpenDAL 那会儿只认
    /// Basic 的行为一致。
    pub fn new(endpoint: &str, root: &str, username: &str, password: &str) -> Result<Self> {
        let client =
            WebdavClient::new(endpoint, username, password, &WebdavSettings::default())?;
        let dir = path::normalize(&format!("/{}", root));
        Ok(Self { client, dir })
    }

    /// 列出落点里直接那一层文件：`(文件名, 大小)`。落点还不存在就是空的 ——
    /// 第一次备份时它本来就不在，那不是错误（同 OpenDAL 那会儿的行为）。
    ///
    /// 副本一律平铺，但落点里可能还有别的东西，只把文件交出去：目录没有大小
    /// 可言，调用方要的是"有哪些备份"。
    pub async fn list(&self) -> Result<Vec<(String, u64)>> {
        let entries = match self.client.list_if_exists(&self.dir).await? {
            Some(entries) => entries,
            None => return Ok(Vec::new()),
        };
        Ok(entries
            .into_iter()
            .filter(|entry| !entry.is_dir)
            .map(|entry| (entry.name, entry.size))
            .collect())
    }

    /// 写一个副本（同名覆盖）。
    pub async fn write(&self, name: &str, payload: Vec<u8>) -> Result<()> {
        // WebDAV 的 `PUT` 不会替你建父目录，父目录不在时服务器回 409。
        // 落点目录第一次用时还没有，先把它（以及它上面的每一级）建出来。
        self.client.create_dir_all(&self.dir).await?;
        self.client.write(&self.target(name), payload).await
    }

    /// 读回一个副本的全部字节。
    pub async fn read(&self, name: &str) -> Result<Vec<u8>> {
        self.client.read(&self.target(name)).await
    }

    /// 删一个副本。
    pub async fn delete(&self, name: &str) -> Result<()> {
        self.client.delete(&self.target(name)).await
    }

    /// 副本的落点路径：`{dir}/{name}`。`name` 只可能是
    /// [`crate::service::BackupService`] 那边过来的纯文件名（它自己把过一道
    /// `is_backup_filename`），这里再做一次词法折叠，防止有人把 `..` 塞进来往上爬。
    fn target(&self, name: &str) -> String {
        path::normalize(&format!("{}/{}", self.dir, name))
    }
}

#[cfg(test)]
#[path = "tests/webdav/store.rs"]
mod tests;
