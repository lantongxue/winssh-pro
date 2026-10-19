//! 下载更新包并校验。
//!
//! 文件落在用户数据目录 `winssh-pro/updates/<版本>/`（与库、Clash 内核同一
//! 座目录）。先下到 `.part` 临时文件，校验通过才改名 —— 中途失败、断电、
//! 用户取消都不会留下一个"看起来是安装包"的半截文件。
//!
//! **下载与校验都在用户态做**，包括 Linux。提权只在 [`super::install`] 那一
//! 步：下载一个大文件不需要 root，让 root 去网络上抓东西才是风险。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use super::check::{AvailableUpdate, ReleaseAsset};

/// 两个数据块之间最多隔多久。只卡"卡住不动"，不卡总时长。
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 进度最快多久汇报一次。
const REPORT_INTERVAL: Duration = Duration::from_millis(120);

/// 下载过程中的事件。界面的进度条与阶段文字都由它驱动。
#[derive(Clone, Debug)]
pub enum DownloadEvent {
    /// 开始下载某个文件。
    Start { asset: String, url: String },
    /// 已下载字节数；`total` 来自 Content-Length，可能没有。
    Progress {
        downloaded: u64,
        total: Option<u64>,
        speed: u64,
    },
    /// 下载完成，正在校验 sha256 与签名。
    Verifying,
    /// 校验通过，返回最终文件路径。
    Done(PathBuf),
}

/// 更新包存放目录。`create` 为真时顺手建出来（权限 0700，Unix 上）。
pub fn update_dir(version: &str, create: bool) -> Result<PathBuf> {
    let dir = winssh_db::db_dir()?
        .join("updates")
        .join(safe_version(version));
    if create {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("创建更新目录失败：{}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            // 只有本人能读：里面是待安装的可执行包。
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    Ok(dir)
}

/// 版本号只留安全字符，别让接口返回的东西拼出一个越界路径。
fn safe_version(version: &str) -> String {
    version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 文件名同理：只取最后一段，去掉任何路径成分。
fn safe_file_name(asset_name: &str) -> String {
    Path::new(asset_name)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty() && name != "." && name != "..")
        .unwrap_or_else(|| "update.bin".to_owned())
}

/// 下载并校验 `update` 选中的产物，返回本地文件的路径。
///
/// 没有签名文件或本机公钥为空时**不安装**：返回错误，让界面退化成"打开
/// 下载页让用户手动装"。这是有意的 —— 自动安装的每一步都要求验签通过。
pub async fn download(
    update: &AvailableUpdate,
    on_progress: impl Fn(DownloadEvent) + Send + Sync + 'static,
) -> Result<PathBuf> {
    let Some(asset) = update.asset.clone() else {
        bail!("没有适用于当前平台的更新包");
    };
    if asset.signature_url.is_empty() || super::signature::public_key().is_none() {
        // 没有签名或客户端还没有公钥：这是"能不能自动装"的问题，不是一次
        // 下载失败。调用方据此退化成手动下载，而不是把错误糊给用户。
        bail!("{}", super::install::NO_SIGNATURE);
    }

    let dir = update_dir(&update.version, true)?;
    let file_name = safe_file_name(&asset.name);
    let final_path = dir.join(&file_name);
    let part_path = dir.join(format!("{file_name}.part"));

    on_progress(DownloadEvent::Start {
        asset: asset.name.clone(),
        url: asset.url.clone(),
    });

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(READ_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent(concat!("winssh-pro/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("构建 HTTP 客户端失败")?;

    let bytes = fetch(&client, &asset, &part_path, &on_progress).await?;
    // 无论校验成败都清掉 .part；失败时不留垃圾。
    let _ = std::fs::remove_file(&part_path);

    on_progress(DownloadEvent::Verifying);
    let signature_text = fetch_signature(&client, &asset).await?;
    super::signature::verify_download(&bytes, &asset.sha256, &signature_text)?;

    std::fs::write(&final_path, &bytes)
        .with_context(|| format!("写入更新包失败：{}", final_path.display()))?;
    on_progress(DownloadEvent::Done(final_path.clone()));
    Ok(final_path)
}

/// 流式下载到 `part_path`，同时累加一份完整内容用于校验。
///
/// 更新包几十 MB 量级（Linux 的 exe 带 GPUI 与各语法高亮约几 MB 到十几 MB），
/// 直接收进内存做校验比再读一遍文件简单，也不会有"校验的和安装的不是同一份
/// 文件"的窗口。
async fn fetch(
    client: &reqwest::Client,
    asset: &ReleaseAsset,
    part_path: &Path,
    on_progress: &(impl Fn(DownloadEvent) + Send + Sync),
) -> Result<Vec<u8>> {
    let response = client
        .get(&asset.url)
        .send()
        .await
        .with_context(|| format!("请求更新包失败：{}", asset.url))?;
    if !response.status().is_success() {
        bail!("下载更新包失败，HTTP 状态码 {}", response.status());
    }

    let total = response.content_length();
    let mut stream = response;
    let mut file =
        std::fs::File::create(part_path).with_context(|| format!("创建临时文件失败：{}", part_path.display()))?;
    // Content-Length 来自网络上的一句话，别拿它直接开一块内存：上限 64 MiB，
    // 超过就让 Vec 自己长。
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(64 * 1024 * 1024) as usize);
    let mut downloaded: u64 = 0;
    let started = Instant::now();
    let mut last_report = started - REPORT_INTERVAL;

    let report = |downloaded: u64| {
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        on_progress(DownloadEvent::Progress {
            downloaded,
            total,
            speed: (downloaded as f64 / elapsed) as u64,
        });
    };
    report(0);

    while let Some(chunk) = stream.chunk().await.context("下载中断")? {
        file.write_all(&chunk)
            .with_context(|| "写入下载数据块失败")?;
        bytes.extend_from_slice(&chunk);
        downloaded += chunk.len() as u64;
        if last_report.elapsed() >= REPORT_INTERVAL {
            last_report = Instant::now();
            report(downloaded);
        }
    }
    file.flush().context("刷新下载文件失败")?;
    report(downloaded);
    Ok(bytes)
}

async fn fetch_signature(client: &reqwest::Client, asset: &ReleaseAsset) -> Result<String> {
    let response = client
        .get(&asset.signature_url)
        .send()
        .await
        .with_context(|| format!("请求签名文件失败：{}", asset.signature_url))?;
    if !response.status().is_success() {
        bail!("下载签名文件失败，HTTP 状态码 {}", response.status());
    }
    response.text().await.context("读取签名文件失败")
}

#[cfg(test)]
#[path = "tests/updater/download.rs"]
mod tests;
