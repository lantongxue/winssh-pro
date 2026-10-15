//! 一台 WebDAV 主机的连接层：建一只 `reqwest_dav::Client`、折算路径、
//! 认对端回的那几种人话。
//!
//! 这里比 [`crate::ftp::client`] 薄得多，因为 WebDAV 是 HTTP：连接复用、
//! keep-alive、超时、重定向都由 `reqwest` 管，我们不用自己养连接池。这一层
//! 只解决 HTTP 不管的那几件事。
//!
//! ## 三套路径
//!
//! 同一个文件在这一层有三种写法，弄混了就是 404：
//!
//! | 写法 | 例子（端点 `https://host/dav`） | 谁用 |
//! | --- | --- | --- |
//! | 面板路径 | `/备份/a.txt` | 面板、服务层、备份 |
//! | 请求路径 | `dav/%E5%A4%87%E4%BB%BD/a.txt` | 交给 `reqwest_dav` 发请求 |
//! | href | `/dav/%E5%A4%87%E4%BB%BD/a.txt` | 服务器在 `PROPFIND` 里回的 |
//!
//! **端点 URL 的路径就是挂载点**（[`WebdavClient::mount`]）：面板里的 `/` 就是
//! 服务器上的 `/dav`。请求路径 = 挂载点 + 面板路径、逐段编码（[`request_path`]）；
//! href 解码后减掉挂载点就回到面板路径（[`panel_of_href`]）。
//!
//! 线上踩过的坑：最早一版把挂载点只用在"减 href"上，发请求时漏拼了，于是
//! `https://host/dav` 这种端点（alist / openlist、坚果云、Nextcloud 都是）上
//! 每个请求都打到站点根，备份列表报 404、文件面板进不去目录。
//!
//! ## 认证
//!
//! `reqwest_dav` 只认 Basic / Digest 两种，[`AuthMode::Auto`] 由这一层探一次
//! `WWW-Authenticate` 头决定走哪一种（[`WebdavClient::detect_auth`]）。

use anyhow::{Context as _, Result, anyhow, bail};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest_dav::types::list_cmd::ListResponse;
use reqwest_dav::{Auth, Client, ClientBuilder, Depth, Error as DavError};
use rust_i18n::t;

use super::settings::{AuthMode, WebdavSettings};
use super::store::WebdavEntry;
use winssh_sftp::path;

/// 建连的上限。不含正文传输 —— 大文件传多久都行，卡死靠取消令牌打断。
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// 路径段里要编码的字符：除了 RFC 3986 的 unreserved（字母数字与 `-._~`）
/// 一律编码。宁可多编：`#` 不编会被当成片段截掉、`?` 会变查询串、`%` 原样
/// 发出去服务器会当成转义，`+` `;` `=` 这些各家服务器的理解也不一。服务器那头
/// 反正都会解码，多编一个字符不出错，少编一个就是找不到文件。
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// 一层包装：`reqwest_dav::Client` 加"面板路径 ↔ 请求路径 ↔ href"的折算。
///
/// 廉价克隆（`reqwest_dav::Client` 内部是 `Arc`，克隆共用同一条连接池）。
#[derive(Clone)]
pub struct WebdavClient {
    inner: Client,
    /// 挂载点：端点 URL 的路径部分，**已解码**、带前导 `/`、不带尾 `/`；
    /// 端点就是站点根时为空串。`https://host/dav/` → `/dav`。
    mount: String,
    /// `Auto` 那一档探出服务器要 Digest 之后，要用它重建一只客户端。
    credentials: Credentials,
}

/// 重建客户端要的那几样：站点、账号密码、TLS 设置。
#[derive(Clone)]
struct Credentials {
    /// `scheme://host[:port]`，不带路径。
    origin: String,
    username: String,
    password: String,
    allow_invalid_certs: bool,
}

impl WebdavClient {
    /// 装出一只客户端。`endpoint` 是那一格 URL（可以不带 scheme、带不带尾斜杠
    /// 都行），它的路径部分就是挂载点（见模块文档）。
    ///
    /// 不发请求 —— 认证方式是不是猜对了、端点通不通，要等第一次请求才知道。
    pub fn new(
        endpoint: &str,
        username: &str,
        password: &str,
        settings: &WebdavSettings,
    ) -> Result<Self> {
        let mut endpoint = endpoint.trim().to_string();
        if endpoint.is_empty() {
            bail!("{}", t!("webdav_err.no_endpoint"));
        }
        if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
            endpoint = format!("https://{endpoint}");
        }
        let url = url::Url::parse(&endpoint).with_context(|| {
            t!("webdav_err.bad_endpoint", endpoint => endpoint.clone()).to_string()
        })?;

        let credentials = Credentials {
            origin: format!("{}://{}", url.scheme(), host_authority(&url)),
            username: username.to_owned(),
            password: password.to_owned(),
            allow_invalid_certs: settings.allow_invalid_certs,
        };
        // `Auto` 起步按 Basic 建 —— 探测要拿着一只能发请求的客户端去问
        // （见 [`Self::detect_auth`]），探出是 Digest 再 [`Self::with_auth`] 重建。
        let inner = build_client(&credentials, settings.auth.auth_or_basic())?;

        Ok(Self {
            inner,
            mount: mount_of(url.path()),
            credentials,
        })
    }

    /// 挂载点（已解码，`/dav` 或空串）。
    pub fn mount(&self) -> &str {
        &self.mount
    }

    /// 换一种认证重建客户端。挂载点、TLS 设置都不动。
    ///
    /// [`AuthMode::Auto`] 的落点：先按 Basic 建一只去探（见 [`Self::detect_auth`]），
    /// 服务器回 `WWW-Authenticate: Digest …` 就用这个换过去。
    pub fn with_auth(&self, auth: AuthMode) -> Result<Self> {
        Ok(Self {
            inner: build_client(&self.credentials, auth)?,
            mount: self.mount.clone(),
            credentials: self.credentials.clone(),
        })
    }

    /// 不带凭据 `OPTIONS` 一下面板路径 `panel`，看服务器要 Basic 还是 Digest。
    ///
    /// 交回 `None` 是"探不出来"（端点不通、没回 `WWW-Authenticate`）——
    /// 调用方按 Basic 走，让第一次真请求去报真正的原因。
    pub async fn detect_auth(&self, panel: &str) -> Option<AuthMode> {
        let url = format!("{}/{}", self.credentials.origin, request_path(&self.mount, panel));
        let response = self
            .inner
            .agent
            .request(reqwest::Method::OPTIONS, url)
            .send()
            .await
            .ok()?;
        if response.status().is_success() {
            return Some(AuthMode::Basic);
        }
        let header = response.headers().get("www-authenticate")?.to_str().ok()?;
        match header.trim_start().to_ascii_lowercase().starts_with("digest") {
            true => Some(AuthMode::Digest),
            false => Some(AuthMode::Basic),
        }
    }

    /// 列一个目录的直接子项（`PROPFIND`，`Depth: 1`）。
    ///
    /// 服务器会把目录自己也列进来，这里滤掉；其余每一条折成面板口径的
    /// [`WebdavEntry`]。目录不存在是错误（面板要说清楚）；备份那边要的是
    /// "不存在就当空的"，用 [`Self::list_if_exists`]。
    pub async fn list(&self, dir: &str) -> Result<Vec<WebdavEntry>> {
        self.list_raw(dir).await.map_err(relevant)
    }

    /// 同 [`Self::list`]，但目录不存在时交回 `None` 而不是报错。
    pub async fn list_if_exists(&self, dir: &str) -> Result<Option<Vec<WebdavEntry>>> {
        match self.list_raw(dir).await {
            Ok(entries) => Ok(Some(entries)),
            Err(error) if is_not_found(&error) => Ok(None),
            Err(error) => Err(relevant(error)),
        }
    }

    async fn list_raw(&self, dir: &str) -> Result<Vec<WebdavEntry>, DavError> {
        let dir = path::normalize(&format!("/{dir}"));
        // 列的一定是目录，带上尾斜杠：有的服务器（Apache、nginx）对不带尾斜杠
        // 的集合先回一个 301，PROPFIND 跟着跳转时方法未必保得住。
        let mut target = request_path(&self.mount, &dir);
        if !target.ends_with('/') {
            target.push('/');
        }
        let responses = self.inner.list_rsp(&target, Depth::Number(1)).await?;
        Ok(responses
            .into_iter()
            .filter_map(|response| entry_of(&self.mount, response))
            .filter(|entry| entry.path != dir)
            .collect())
    }

    /// 存不存在。"对端说没有这个路径"是 `Ok(false)`，不是错误 —— 与 S3 那边
    /// `stat` 的口径一致；真错误（连不上、401、403、405）照旧冒泡。
    pub async fn exists(&self, target: &str) -> Result<bool> {
        match self.probe(target).await {
            Ok(_) => Ok(true),
            Err(error) if is_not_found(&error) => Ok(false),
            Err(error) => Err(relevant(error)),
        }
    }

    /// `PROPFIND`，`Depth: 0`：只问这一个路径自己是什么。
    ///
    /// 交回原始 [`DavError`]，好让调用方用 [`is_not_found`] 认出"不存在"。
    /// 服务器回的那一条找不到（极少见）时交回 `None`。
    pub async fn probe(&self, target: &str) -> Result<Option<WebdavEntry>, DavError> {
        let target = path::normalize(&format!("/{target}"));
        let responses = self
            .inner
            .list_rsp(&request_path(&self.mount, &target), Depth::Number(0))
            .await?;
        Ok(responses
            .into_iter()
            .filter_map(|response| entry_of(&self.mount, response))
            .find(|entry| entry.path == target))
    }

    /// 建目录（`MKCOL`）。父目录不在时服务器回 409，见 [`Self::create_dir_all`]。
    pub async fn create_dir(&self, target: &str) -> Result<()> {
        self.inner
            .mkcol(&request_path(&self.mount, target))
            .await
            .map_err(relevant)
    }

    /// 逐级建目录，已经在的跳过。WebDAV 的 `MKCOL` 只建一级（父目录不在回 409），
    /// 对着已存在的目录回 405 —— 所以先问一句在不在，不在再建。
    pub async fn create_dir_all(&self, target: &str) -> Result<()> {
        let target = path::normalize(&format!("/{target}"));
        let mut current = String::new();
        for segment in target.split('/').filter(|segment| !segment.is_empty()) {
            current = format!("{current}/{segment}");
            if !self.exists(&current).await? {
                self.create_dir(&current).await?;
            }
        }
        Ok(())
    }

    /// 整份读一个文件。
    pub async fn read(&self, target: &str) -> Result<Vec<u8>> {
        let response = self
            .inner
            .get(&request_path(&self.mount, target))
            .await
            .map_err(relevant)?;
        let bytes = response.bytes().await.map_err(|error| anyhow!(error.to_string()))?;
        Ok(bytes.to_vec())
    }

    /// 打开一条下载正文的流。调用方一块一块读，不把整个文件读进内存。
    pub async fn read_stream(
        &self,
        target: &str,
    ) -> Result<impl futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>>> {
        let response = self
            .inner
            .get_raw(&request_path(&self.mount, target))
            .await
            .map_err(relevant)?;
        // 状态码不是 2xx 时（对端把错误页当正文回下来）不该往下走。
        let status = response.status();
        if !status.is_success() {
            bail!("{}", describe_status(status.as_u16()));
        }
        Ok(response.bytes_stream())
    }

    /// 整份写一个文件（覆盖已有的）。
    pub async fn write(&self, target: &str, bytes: Vec<u8>) -> Result<()> {
        self.inner
            .put(&request_path(&self.mount, target), bytes)
            .await
            .map_err(relevant)
    }

    /// 流式写一个文件：正文是一条流，一块一块往前送，不把整个文件读进内存。
    pub async fn write_stream(&self, target: &str, body: reqwest::Body) -> Result<()> {
        let response = self
            .inner
            .put_raw(&request_path(&self.mount, target), body)
            .await
            .map_err(relevant)?;
        let status = response.status();
        if !status.is_success() {
            bail!("{}", describe_status(status.as_u16()));
        }
        Ok(())
    }

    /// 删一个条目。目录与文件都走 `DELETE`（WebDAV 的目录删除是递归的，
    /// RFC 4918 如此）。
    pub async fn delete(&self, target: &str) -> Result<()> {
        self.inner
            .delete(&request_path(&self.mount, target))
            .await
            .map_err(relevant)
    }

    /// 改名或移动（`MOVE`），**不覆盖**已有目标（`Overwrite: F`，对端回 412）。
    ///
    /// 不用 `reqwest_dav::Client::mv`：它把目标原样塞进 `Destination` 头 ——
    /// 中文名直接是一个非法头值，`#`、空格也没编码；而且给的是一个相对路径，
    /// RFC 4918 要的是绝对 URI，Apache 一类较真的服务器会拒。这里借它的
    /// `start_request`（认证照样由它加，Digest 也行），头自己填。
    pub async fn r#move(&self, from: &str, to: &str) -> Result<()> {
        let destination = format!(
            "{}/{}",
            self.credentials.origin,
            request_path(&self.mount, to)
        );
        let method = reqwest::Method::from_bytes(b"MOVE").expect("MOVE 是合法的方法名");
        let response = self
            .inner
            .start_request(method, &request_path(&self.mount, from))
            .await
            .map_err(relevant)?
            .header("Destination", destination)
            .header("Overwrite", "F")
            .send()
            .await
            .map_err(|error| relevant(DavError::Reqwest(error)))?;
        let status = response.status();
        if !status.is_success() {
            bail!("{}", describe_status(status.as_u16()));
        }
        Ok(())
    }
}

/// 端点 URL 的路径折成挂载点：解码、折叠、去掉尾斜杠，站点根是空串。
pub fn mount_of(url_path: &str) -> String {
    let normalized = path::normalize(&format!("/{}", percent_decode(url_path)));
    match normalized.as_str() {
        "/" => String::new(),
        _ => normalized,
    }
}

/// 面板路径 → 请求路径：拼上挂载点、逐段编码，不带前导 `/`
/// （`reqwest_dav` 自己会在 origin 与它之间补一个）。
///
/// `/dav` + `/备份/a b.txt` → `dav/%E5%A4%87%E4%BB%BD/a%20b.txt`。
pub fn request_path(mount: &str, panel: &str) -> String {
    let full = path::normalize(&format!("{mount}/{panel}"));
    full.split('/')
        .filter(|segment| !segment.is_empty())
        .map(percent_encode)
        .collect::<Vec<_>>()
        .join("/")
}

/// 服务器回的 href → 面板路径：解码、去掉 scheme 与主机、减掉挂载点、折叠。
///
/// 挂载点按**整段**减：挂载点 `/dav` 不能把 `/dav2/x` 减成 `2/x`。减不掉的
/// （服务器回了挂载点之外的路径，不该发生）原样当面板路径用。
pub fn panel_of_href(mount: &str, href: &str) -> String {
    let href = href.trim();
    // href 可能是完整的 `https://host/dav/a`，也可能只是 `/dav/a`。
    let raw_path = match href.find("://") {
        Some(scheme) => match href[scheme + 3..].find('/') {
            Some(index) => &href[scheme + 3 + index..],
            None => "/",
        },
        None => href,
    };
    let decoded = path::normalize(&format!("/{}", percent_decode(raw_path)));
    if mount.is_empty() {
        return decoded;
    }
    match decoded.strip_prefix(mount) {
        Some("") => "/".to_string(),
        Some(rest) if rest.starts_with('/') => rest.to_string(),
        _ => decoded,
    }
}

/// 主机表单里那格"根目录"折成面板里的起始目录。
///
/// 那一格是面板路径（相对挂载点）。但用户常常把服务器上的完整路径填进来 ——
/// 端点 `https://host/dav`、根目录 `/dav/备份`，照字面拼就成了 `/dav/dav/备份`。
/// 填的路径以挂载点打头时，把挂载点剥掉。
pub fn home_within(mount: &str, root: &str) -> String {
    let root = path::normalize(&format!("/{}", root.trim()));
    if mount.is_empty() {
        return root;
    }
    match root.strip_prefix(mount) {
        Some("") => "/".to_string(),
        Some(rest) if rest.starts_with('/') => rest.to_string(),
        _ => root,
    }
}

/// `PROPFIND` 回的一条折成面板条目。
///
/// 不用 `reqwest_dav` 的 `ListEntity::try_from`：它要求文件必须带
/// `getlastmodified`，缺了就整条报错 —— 那样一个怪条目就会让文件从列表里
/// 悄悄消失。这里只要求有一个 2xx 的 propstat，其余字段缺了按空处理。
fn entry_of(mount: &str, response: ListResponse) -> Option<WebdavEntry> {
    let path = panel_of_href(mount, &response.href);
    let prop = response
        .prop_stat
        .into_iter()
        .find(|prop_stat| status_is_ok(&prop_stat.status))?
        .prop;
    let is_dir = prop.resource_type.collection.is_some();
    let name = match path.as_str() {
        "/" => "/".to_string(),
        _ => path::file_name(&path).to_string(),
    };
    Some(WebdavEntry {
        name,
        path,
        is_dir,
        size: match is_dir {
            true => 0,
            false => prop.content_length.unwrap_or(0).max(0) as u64,
        },
        modified: prop.last_modified.and_then(system_time),
    })
}

/// propstat 的状态行（`HTTP/1.1 200 OK`）是不是 2xx。
fn status_is_ok(status: &str) -> bool {
    status
        .split_whitespace()
        .nth(1)
        .is_some_and(|code| code.starts_with('2'))
}

/// 照凭据与认证档建一只 `reqwest_dav::Client`。
fn build_client(credentials: &Credentials, auth: AuthMode) -> Result<Client> {
    let auth = match auth {
        AuthMode::Basic | AuthMode::Auto => {
            Auth::Basic(credentials.username.clone(), credentials.password.clone())
        }
        AuthMode::Digest => {
            Auth::Digest(credentials.username.clone(), credentials.password.clone())
        }
    };
    // 不借系统代理：用户可能正开着 Clash 的系统代理，我们要的是确定的连法
    // （同 `src/clash/` 那两处 `no_proxy()` 的理由）。
    let agent = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .danger_accept_invalid_certs(credentials.allow_invalid_certs)
        .no_proxy()
        .build()
        .context("构建 WebDAV HTTP 客户端失败")?;

    ClientBuilder::new()
        .set_agent(agent)
        .set_host(credentials.origin.clone())
        .set_auth(auth)
        .build()
        .map_err(|error| anyhow!(describe(&error)))
}

/// `scheme://host:port` 里的 `host:port` 那一段（`url` 已经拆好了）。
fn host_authority(url: &url::Url) -> String {
    match url.port() {
        Some(port) => format!("{}:{}", url.host_str().unwrap_or_default(), port),
        None => url.host_str().unwrap_or_default().to_string(),
    }
}

/// `chrono` 的时间转 `SystemTime`（面板那一列要的）。
fn system_time(time: chrono::DateTime<chrono::Utc>) -> Option<std::time::SystemTime> {
    let secs = u64::try_from(time.timestamp()).ok()?;
    Some(
        std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(secs)
            + std::time::Duration::from_nanos(u64::from(time.timestamp_subsec_nanos())),
    )
}

/// 百分号解码。解不动（名字里真有个孤立的 `%`）就原样返回 —— 宁可显示
/// 一个带 `%` 的名字，也不能因为一个 href 解不开就整条列不出来。
pub fn percent_decode(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|_| raw.to_string())
}

/// 百分号编码一个 URL 路径段（不含 `/`），编码集见 [`PATH_SEGMENT`]。
pub fn percent_encode(segment: &str) -> String {
    utf8_percent_encode(segment, PATH_SEGMENT).to_string()
}

/// 一个 HTTP 状态码说成一句人话。
fn describe_status(code: u16) -> String {
    match code {
        401 => t!("webdav_err.unauthorized").to_string(),
        403 => t!("webdav_err.forbidden").to_string(),
        404 => t!("webdav_err.not_found").to_string(),
        405 => t!("webdav_err.method_not_allowed").to_string(),
        409 => t!("webdav_err.conflict").to_string(),
        412 => t!("webdav_err.precondition_failed").to_string(),
        507 => t!("webdav_err.insufficient_storage").to_string(),
        code => t!("webdav_err.status", code => code.to_string()).to_string(),
    }
}

/// 让 `reqwest_dav` 的错误说成一行人话，必要时带上状态码。
///
/// 它的 `Error` 只是 `Display` 出来一个 `debug_struct`（`reqwest_dav::Error { kind:
/// "Decode", source: StatusMismatched(…) }`），直接甩给用户太糙；这里认出那几种
/// 常见的情况说清楚，认不出就把原话附上。
pub fn describe(error: &DavError) -> String {
    match error {
        DavError::Decode(reqwest_dav::DecodeError::StatusMismatched(mismatch)) => {
            describe_status(mismatch.response_code)
        }
        DavError::Decode(reqwest_dav::DecodeError::Server(server)) => match server.response_code {
            // 认得的状态码说人话；服务器另外附的正文多半是一页 HTML，不往界面上贴。
            401 | 403 | 404 | 405 | 409 | 412 | 507 => describe_status(server.response_code),
            code => t!(
                "webdav_err.server",
                code => code.to_string(),
                message => server.message.chars().take(200).collect::<String>(),
            )
            .to_string(),
        },
        DavError::Reqwest(source) if source.is_timeout() => t!("webdav_err.timeout").to_string(),
        DavError::Reqwest(source) if source.is_connect() => {
            t!("webdav_err.connect", error => source.to_string()).to_string()
        }
        other => format!("{other}"),
    }
}

/// 这个错误是不是"对端说没有这个路径"（404）。
///
/// 只认 404。405 是"这个地址不接受 WebDAV 方法" —— 端点没指到挂载点时
/// 站点根回的就是它；把它当成"不存在"，备份列表就会悄悄变成空的，用户
/// 只会以为自己还没备份过。
pub fn is_not_found(error: &DavError) -> bool {
    match error {
        DavError::Decode(reqwest_dav::DecodeError::StatusMismatched(mismatch)) => {
            mismatch.response_code == 404
        }
        DavError::Decode(reqwest_dav::DecodeError::Server(server)) => server.response_code == 404,
        _ => false,
    }
}

/// 把 `reqwest_dav` 的错误折成 `anyhow`。不加"动作"前缀 —— 那是服务层的事。
fn relevant(error: DavError) -> anyhow::Error {
    anyhow!(describe(&error))
}

#[cfg(test)]
#[path = "tests/webdav/client.rs"]
mod tests;
