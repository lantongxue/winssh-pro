//! 检查更新：请求云函数，解析出"有没有新版、该下哪个文件"。
//!
//! 接口由 winssh.net 的云函数提供（`cloud-functions/update.go`），它负责
//! 版本比较、挑出目标平台的产物、并把 GitHub Release 的正文（发版日志）原样
//! 带回来。客户端不直连 GitHub：匿名 API 每个 IP 每小时只有 60 次，国内访问
//! 也不稳定。
//!
//! 调试时可以用环境变量临时指向别的地址（同样在编译期生效）：
//!
//! ```text
//! WINSSH_UPDATE_ENDPOINT=http://127.0.0.1:9000/v1/update cargo run
//! ```

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

/// 更新检查接口。部署在 winssh.net（EdgeOne Pages 的云函数）。
///
/// 本地开发想指向别的地址时，构建前设 `WINSSH_UPDATE_ENDPOINT`。
const DEFAULT_ENDPOINT: &str = "https://winssh.net/api/v1/update";

/// 实际使用的接口地址。
pub fn endpoint() -> &'static str {
    option_env!("WINSSH_UPDATE_ENDPOINT").unwrap_or(DEFAULT_ENDPOINT)
}

/// 建立连接最多等多久（与 mihomo 下载器同一档）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// 整个请求最多等多久。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// 当前平台，取值与云函数一致：`windows` / `macos` / `linux`。
pub fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        "linux" => "linux",
        other => other,
    }
}

/// 当前架构，取值与云函数一致：`x64` / `arm64`。
pub fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// 接口返回的单个可下载文件。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ReleaseAsset {
    /// `installer` / `portable` / `dmg` / `archive` / `deb` / `rpm`。
    pub kind: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    /// 十六进制 sha256；服务端拿不到 GitHub 的 digest 时为空。
    #[serde(default)]
    pub sha256: String,
    /// `.minisig` 的地址；没签名时为空。
    #[serde(default)]
    pub signature_url: String,
}

/// 有新版本时的那一份信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableUpdate {
    /// 新版本号（不带 `v`，如 `0.0.6`）。
    pub version: String,
    /// 发版日志，Markdown 原文（git-cliff 生成的那份）。
    pub notes: String,
    pub published_at: String,
    /// GitHub release 页面，用于"打开发布页"。
    pub release_url: String,
    /// 按当前平台/架构挑出来的产物。形状不认识时为 `None`。
    pub asset: Option<ReleaseAsset>,
}

/// 接口原始响应。
#[derive(Debug, Deserialize)]
struct Response {
    update_available: bool,
    #[serde(default)]
    latest: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    release_url: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

/// 检查有没有更新。`current` 是当前版本号（`env!("CARGO_PKG_VERSION")`）。
///
/// 返回 `Ok(None)` 表示已经是最新（或被跳过、或被接口明确否定）；网络/接口
/// 出错才返回 `Err`，让调用方决定要不要提示 —— "检查失败"和"没有更新"必须
/// 分得开，否则用户永远不知道自动检查是不是真的在工作。
///
/// 预发布构建（`0.0.5-dev.abc1234`）不检查：它们本来就比任何正式版"低"，
/// 检查出来的永远是"有新版本"，只会打扰开发者。
pub async fn check(current: &str) -> Result<Option<AvailableUpdate>> {
    let current_version = super::semver::Version::parse(current)
        .with_context(|| format!("当前版本号 `{current}` 解析不了"))?;
    if current_version.is_prerelease() {
        return Ok(None);
    }

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(10))
        // GitHub 与 Cloudflare 都会拒绝没有 User-Agent 的请求。
        .user_agent(concat!("winssh-pro/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("构建 HTTP 客户端失败")?;

    // 不引 reqwest 的 `query` 特性（只为这一个查询串多开一份 URL 编码依赖不划算）：
    // `current` 已经过 semver 解析，只会是数字、点和 `-`/`+`，os/arch 是编译期
    // 常量，都不需要转义。
    let url = format!(
        "{}?version={}&os={}&arch={}",
        endpoint(),
        current,
        platform(),
        arch()
    );
    let response = client
        .get(url)
        .send()
        .await
        .context("请求更新检查接口失败")?;

    let status = response.status();
    if !status.is_success() {
        bail!("更新检查接口返回 HTTP {status}");
    }
    let body = response
        .bytes()
        .await
        .context("读取更新检查接口响应失败")?;
    let parsed: Response =
        serde_json::from_slice(&body).context("更新检查接口返回了无法解析的 JSON")?;
    if !parsed.update_available {
        return Ok(None);
    }
    if parsed.latest.is_empty() {
        bail!("更新检查接口说有新版本，却没给版本号");
    }

    let latest = parsed
        .latest
        .trim_start_matches('v')
        .to_owned();
    // 服务端已经比较过，这里再比一次只是防御：接口缓存过期时可能把旧版本
    // 当成"最新"返回。
    let latest_version = super::semver::Version::parse(&latest)
        .with_context(|| format!("接口返回的版本号 `{latest}` 解析不了"))?;
    if super::semver::compare(&latest_version, &current_version) != std::cmp::Ordering::Greater {
        return Ok(None);
    }

    Ok(Some(AvailableUpdate {
        version: latest,
        notes: parsed.notes,
        published_at: parsed.published_at,
        release_url: parsed.release_url,
        asset: pick_asset(&parsed.assets),
    }))
}

/// 从接口给的产物里挑当前平台该用的那个。
///
/// kind 的优先级就是各平台安装形态的偏好：
/// - Windows：安装版（`installer`）优先，其次免安装（`portable`）；
/// - macOS：`dmg` 是给手动安装的，自动更新用 `archive` 里的 `.app`；
/// - Linux：看本机是 deb 还是 rpm 系，另一边只作为兜底（真装上会失败，
///   界面会先做归属检查）。
fn pick_asset(assets: &[ReleaseAsset]) -> Option<ReleaseAsset> {
    let preferred: &[&str] = match platform() {
        // 免安装版（tar.gz 解出来的）旁边没有 Inno 的卸载器：那时候选中
        // installer 会静默装一份到 Program Files，和用户手上这份并存 ——
        // 所以先挑 portable，让安装计划落到"只提示、手动下载"。
        "windows" if !super::install::is_installed_build() => &["portable", "installer"],
        "windows" => &["installer", "portable"],
        "macos" => &["archive", "dmg"],
        // deb 与 rpm 系各认自己那一个；认不出包管理器时按 deb 优先 ——
        // 两种都装上会失败，界面在安装前还会做一次归属检查。
        _ if linux_prefers_rpm() => &["rpm", "deb", "archive"],
        _ => &["deb", "rpm", "archive"],
    };
    preferred
        .iter()
        .find_map(|kind| assets.iter().find(|asset| asset.kind == *kind))
        .cloned()
}

/// 本机是 rpm 系（有 `rpm` 而没有 `dpkg`）吗。
#[cfg(target_os = "linux")]
fn linux_prefers_rpm() -> bool {
    let on_path = |program: &str| {
        std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(program))
                .any(|candidate| candidate.is_file())
        })
    };
    !on_path("dpkg") && on_path("rpm")
}

#[cfg(not(target_os = "linux"))]
fn linux_prefers_rpm() -> bool {
    false
}

#[cfg(test)]
#[path = "tests/updater/check.rs"]
mod tests;
