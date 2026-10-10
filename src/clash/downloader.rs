//! 自动检查与下载 mihomo（Clash.Meta）内核。
//!
//! 在启动 Clash 代理时，若本地尚未找到 mihomo 可执行文件，
//! 本模块会自动根据当前运行的操作系统及 CPU 指令集架构，
//! 从官方 GitHub Releases 下载匹配版本的二进制压缩包并解压安装。
//!
//! - Release 基准地址：<https://github.com/MetaCubeX/mihomo/releases/tag/v1.19.32>
//! - 架构匹配：
//!   - Darwin (macOS): `darwin-arm64` / `darwin-amd64` (格式形如 `mihomo-darwin-arm64-v1.19.32.gz`)
//!   - Linux: `linux-amd64` / `linux-arm64` / `linux-386` / `linux-armv7` 等 (.gz)
//!   - Windows: `windows-amd64` / `windows-arm64` / `windows-386` (.zip)
//! - 存放路径：用户数据目录 `clash/bin/mihomo` (Windows 为 `mihomo.exe`)
//!
//! ## 谁来决定下载
//!
//! 下载是个**要用户点头**的动作（几十 MB 的外网流量，还会往数据目录写一个
//! 可执行文件），所以 [`crate::clash::runtime`] 起实例时找不到内核**不会**
//! 自己去下，而是报"没找到"；界面在点启动之前先问一遍
//! （见 `crate::ui::dialogs::MihomoDownloadDialog`），用户确认后调 [`begin`]，
//! 进度走 [`subscribe`] 那条进程级的 [`watch`] 通道 —— 和
//! [`crate::clash::runtime`] 的状态通道同一种做法：谁想看谁订阅，随时订阅都先
//! 拿到**此刻**那一份。

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use gpui_kit::SharedString;
use tokio::sync::watch;

/// 当前指定的 mihomo 版本 Tag。
pub const MIHOMO_TAG: &str = "v1.19.32";

/// 建立连接最多等多久。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// 两个数据块之间最多隔多久。只卡**卡住不动**，不卡总时长 ——
/// 慢线路上几十 MB 本来就要几分钟，总时长超时会把一次正常的下载掐断。
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// 进度最快多久汇报一次。按百分比汇报的话，不知道总大小时一次都不会发。
const REPORT_INTERVAL: Duration = Duration::from_millis(120);

/// 下载进度或状态事件。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum DownloadEvent {
    /// 开始下载某个 release 资源。
    Start { asset: String, url: String },
    /// 下载进度。
    Progress {
        downloaded: u64,
        total: Option<u64>,
        percent: Option<u8>,
        /// 到目前为止的平均速度，字节每秒。
        speed: u64,
    },
    /// 下载完成，正在解压与配置可执行文件。
    Unpacking,
    /// 准备完成，返回最终的可执行文件路径。
    Done(PathBuf),
}

/// 下载中途的临时压缩包。失败、取消时要靠它找到并清掉。
fn temp_archive_path(target_dir: &Path, asset: &str) -> PathBuf {
    target_dir.join(format!("{asset}.download.tmp"))
}

/// 获取当前操作系统对应的 mihomo release 名称组件。
pub fn detect_os() -> Result<&'static str> {
    match std::env::consts::OS {
        "macos" => Ok("darwin"),
        "linux" => Ok("linux"),
        "windows" => Ok("windows"),
        "freebsd" => Ok("freebsd"),
        other => bail!("暂不支持的操作系统：{other}"),
    }
}

/// 获取当前 CPU 架构对应的 mihomo release 名称组件。
pub fn detect_arch() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        "x86" => Ok("386"),
        "arm" => Ok("armv7"),
        "riscv64" => Ok("riscv64"),
        "loongarch64" => Ok("loong64-abi1"),
        other => bail!("暂不支持的 CPU 架构：{other}"),
    }
}

/// 获取当前平台对应的压缩包扩展名（Windows 为 zip，Unix/Linux/macOS 为 gz）。
pub fn detect_extension() -> &'static str {
    if std::env::consts::OS == "windows" {
        "zip"
    } else {
        "gz"
    }
}

/// 根据系统和架构自动计算 release 资源文件名。
/// 例如：`mihomo-darwin-arm64-v1.19.32.gz` 或 `mihomo-windows-amd64-v1.19.32.zip`。
pub fn release_asset_name(tag: &str) -> Result<String> {
    let os = detect_os()?;
    let arch = detect_arch()?;
    let ext = detect_extension();
    Ok(format!("mihomo-{os}-{arch}-{tag}.{ext}"))
}

/// 计算 GitHub release 的直接下载 URL。
pub fn download_url(tag: &str) -> Result<String> {
    let asset = release_asset_name(tag)?;
    Ok(format!(
        "https://github.com/MetaCubeX/mihomo/releases/download/{tag}/{asset}"
    ))
}

/// mihomo 二进制默认保存目录：`db::db_dir()/clash/bin`。
pub fn default_binary_dir() -> Result<PathBuf> {
    let dir = super::runtime::clash_dir()?.join("bin");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建 mihomo 二进制目录失败：{}", dir.display()))?;
    Ok(dir)
}

/// mihomo 二进制默认保存文件全路径。
#[allow(dead_code)]
pub fn default_binary_path() -> Result<PathBuf> {
    let dir = default_binary_dir()?;
    Ok(dir.join(super::runtime::MIHOMO_EXE))
}

/// 检查 mihomo 是否存在，如果不存在则自动下载解压安装。
#[allow(dead_code)]
pub async fn ensure_binary() -> Result<PathBuf> {
    ensure_binary_with_progress(|_| {}).await
}

/// 检查 mihomo 是否存在，带事件回调进度通知。
pub async fn ensure_binary_with_progress<F>(on_progress: F) -> Result<PathBuf>
where
    F: Fn(DownloadEvent) + Send + Sync + 'static,
{
    if let Ok(existing) = super::runtime::binary_path() {
        return Ok(existing);
    }
    download_mihomo(on_progress).await
}

/// 执行下载并解压安装 mihomo。
pub async fn download_mihomo<F>(on_progress: F) -> Result<PathBuf>
where
    F: Fn(DownloadEvent) + Send + Sync + 'static,
{
    let tag = MIHOMO_TAG;
    let asset = release_asset_name(tag)?;
    let url = download_url(tag)?;
    let target_dir = default_binary_dir()?;
    let final_bin = target_dir.join(super::runtime::MIHOMO_EXE);

    on_progress(DownloadEvent::Start {
        asset: asset.clone(),
        url: url.clone(),
    });

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("winssh-pro")
        .build()
        .context("构建 HTTP 客户端失败")?;

    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("请求下载地址失败: {url}"))?;

    if !response.status().is_success() {
        bail!("下载 mihomo 失败，HTTP 状态码: {}", response.status());
    }

    let total_size = response.content_length();
    let temp_archive = temp_archive_path(&target_dir, &asset);

    // 流式下载到临时压缩文件
    {
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::File::create(&temp_archive)
            .await
            .with_context(|| format!("创建临时文件失败: {}", temp_archive.display()))?;

        let mut stream = response;
        let mut downloaded: u64 = 0;
        let started = Instant::now();
        let mut last_report = started - REPORT_INTERVAL;

        let report = |downloaded: u64| {
            let pct = total_size.map(|total| {
                if total > 0 {
                    ((downloaded as f64 / total as f64) * 100.0).min(100.0) as u8
                } else {
                    0
                }
            });
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            on_progress(DownloadEvent::Progress {
                downloaded,
                total: total_size,
                percent: pct,
                speed: (downloaded as f64 / elapsed) as u64,
            });
        };

        // 零字节的那一下也报：进度条要立刻从"连接中"换成真正的数字。
        report(0);
        while let Some(chunk) = stream.chunk().await.context("下载中断")? {
            downloaded += chunk.len() as u64;
            file.write_all(&chunk)
                .await
                .with_context(|| "写入下载数据块失败")?;

            if last_report.elapsed() >= REPORT_INTERVAL {
                last_report = Instant::now();
                report(downloaded);
            }
        }

        file.flush().await?;
        // 收尾那一份一定发出去：节流可能刚好把最后几个块吞了。
        report(downloaded);
    }

    on_progress(DownloadEvent::Unpacking);

    let temp_bin = target_dir.join(format!("{}.unpack.tmp", super::runtime::MIHOMO_EXE));

    // 解压处理：.gz 或 .zip
    let unpack_result = (|| -> Result<()> {
        let is_zip = asset.ends_with(".zip");
        if is_zip {
            unpack_zip(&temp_archive, &temp_bin)?;
        } else {
            unpack_gz(&temp_archive, &temp_bin)?;
        }
        Ok(())
    })();

    // 无论解压成功与否，清理临时压缩包
    let _ = std::fs::remove_file(&temp_archive);
    unpack_result?;

    // Unix 平台赋予可执行权限 0o755
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&temp_bin)
            .with_context(|| "读取临时可执行文件权限失败")?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temp_bin, perms).with_context(|| "设置可执行权限失败")?;
    }

    // Windows 平台如果存在已有的目标文件，需先清理再移动
    #[cfg(windows)]
    if final_bin.exists() {
        let _ = std::fs::remove_file(&final_bin);
    }

    // 原子置换为最终目标文件
    std::fs::rename(&temp_bin, &final_bin)
        .with_context(|| format!("移动二进制文件到最终路径失败: {}", final_bin.display()))?;

    // Windows 平台：如果旁边有 wintun.dll，同步拷贝到 bin 目录下
    copy_wintun_if_needed(&target_dir);

    on_progress(DownloadEvent::Done(final_bin.clone()));
    log::info!("mihomo 内核下载解压完成：{}", final_bin.display());
    Ok(final_bin)
}

/// 解压 .gz 文件（解压后内容直接为单个可执行二进制）。
fn unpack_gz(src: &Path, dst: &Path) -> Result<()> {
    let in_file =
        std::fs::File::open(src).with_context(|| format!("打开压缩文件失败: {}", src.display()))?;
    let mut decoder = flate2::read::GzDecoder::new(in_file);
    let mut out_file = std::fs::File::create(dst)
        .with_context(|| format!("创建目标文件失败: {}", dst.display()))?;
    std::io::copy(&mut decoder, &mut out_file).with_context(|| "解压 gzip 数据失败")?;
    Ok(())
}

/// 解压 .zip 文件（从包中提取出 .exe 可执行文件）。
fn unpack_zip(src: &Path, dst: &Path) -> Result<()> {
    let in_file = std::fs::File::open(src)
        .with_context(|| format!("打开 zip 文件失败: {}", src.display()))?;
    let mut archive = zip::ZipArchive::new(in_file).with_context(|| "读取 zip 压缩包失败")?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .with_context(|| format!("读取 zip 内文件索引 {i} 失败"))?;
        let name = file.name().to_string();
        if name.ends_with(".exe") {
            let mut out_file = std::fs::File::create(dst)
                .with_context(|| format!("创建目标文件失败: {}", dst.display()))?;
            std::io::copy(&mut file, &mut out_file).with_context(|| "从 zip 提取 exe 失败")?;
            return Ok(());
        }
    }
    bail!("在 zip 压缩包中未找到任何可执行文件 (.exe)");
}

/// Windows 平台复制 wintun.dll 到 mihomo 目录（虚拟网卡依赖）。
#[cfg(windows)]
fn copy_wintun_if_needed(target_dir: &Path) {
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        let src = exe_dir.join("wintun.dll");
        let dst = target_dir.join("wintun.dll");
        if src.is_file() && !dst.exists() {
            if let Err(e) = std::fs::copy(&src, &dst) {
                log::warn!("复制 wintun.dll 到 mihomo 目录失败：{e}");
            }
        }
    }
}

#[cfg(not(windows))]
fn copy_wintun_if_needed(_target_dir: &Path) {}

/// 这次下载走到哪一步了。
#[derive(Clone, Debug, PartialEq)]
pub enum DownloadPhase {
    /// 没有下载在进行。
    Idle,
    /// 在下：连接、收数据。
    Downloading,
    /// 数据收完了，正在解压并就位。这一步取消不了，也不需要 —— 几百毫秒。
    Unpacking,
    /// 装好了。
    Done,
    /// 没下成。带着原因。
    Failed(SharedString),
}

impl DownloadPhase {
    /// 下载此刻是不是在占着（不能再起第二份）。
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Downloading | Self::Unpacking)
    }
}

/// 此刻的样子。弹窗每次重画读的就是它。
#[derive(Clone, Debug, PartialEq)]
pub struct DownloadState {
    pub phase: DownloadPhase,
    /// 在下的那个文件名。
    pub asset: SharedString,
    pub downloaded: u64,
    /// 服务器没报大小时是 `None`，进度条退回\"不定长\"那一档。
    pub total: Option<u64>,
    /// 平均速度，字节每秒。
    pub speed: u64,
}

impl Default for DownloadState {
    fn default() -> Self {
        Self {
            phase: DownloadPhase::Idle,
            asset: SharedString::default(),
            downloaded: 0,
            total: None,
            speed: 0,
        }
    }
}

impl DownloadState {
    /// 已完成的比例，0.0 ~ 1.0。不知道总大小时是 `None`。
    pub fn fraction(&self) -> Option<f32> {
        match self.total {
            Some(total) if total > 0 => {
                Some((self.downloaded as f64 / total as f64).min(1.0) as f32)
            }
            _ => None,
        }
    }
}

static CHANGES: LazyLock<watch::Sender<Arc<DownloadState>>> =
    LazyLock::new(|| watch::channel(Arc::new(DownloadState::default())).0);

/// 在跑的那个下载任务。`cancel` 靠它打断。
static TASK: LazyLock<Mutex<Option<tokio::task::JoinHandle<()>>>> = LazyLock::new(Default::default);

/// 此刻那一份。
pub fn state() -> Arc<DownloadState> {
    CHANGES.borrow().clone()
}

/// 订阅变化。拿到手先读一次值，再等 `changed()`。
pub fn subscribe() -> watch::Receiver<Arc<DownloadState>> {
    CHANGES.subscribe()
}

fn publish(edit: impl FnOnce(&mut DownloadState)) {
    CHANGES.send_modify(|current| {
        let mut next = DownloadState::clone(current);
        edit(&mut next);
        *current = Arc::new(next);
    });
}

/// 内核在不在（本地能不能找到可执行文件）。界面起实例之前先问它。
pub fn is_installed() -> bool {
    super::runtime::binary_path().is_ok()
}

/// 将要下载的是哪个文件、从哪来、存到哪去。确认弹窗把这三样摆给用户看。
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub asset: String,
    pub url: String,
    pub target: PathBuf,
}

/// 重置下载状态为初始空闲。
pub fn reset() {
    publish(|state| *state = DownloadState::default());
}

/// 算出这台机器上的下载计划。系统或架构不在 mihomo 的发布清单里时报错。
pub fn plan() -> Result<Plan> {
    Ok(Plan {
        asset: release_asset_name(MIHOMO_TAG)?,
        url: download_url(MIHOMO_TAG)?,
        target: default_binary_dir()?.join(super::runtime::MIHOMO_EXE),
    })
}

/// 开始下载。已经在下了就什么也不做。进度走 [`subscribe`]。
///
/// 任务跑在应用级 runtime 上：弹窗关掉（比如用户点了\"后台\"之类）不该把它
/// 连带掐断；真要停就明着调 [`cancel`]。
pub fn begin() {
    if state().phase.is_busy() {
        return;
    }
    let asset = release_asset_name(MIHOMO_TAG).unwrap_or_default();
    publish(|state| {
        *state = DownloadState {
            phase: DownloadPhase::Downloading,
            asset: asset.clone().into(),
            ..DownloadState::default()
        };
    });
    let handle = crate::db::runtime().spawn(async move {
        let outcome = download_mihomo(|event| match event {
            DownloadEvent::Start { .. } => {}
            DownloadEvent::Progress {
                downloaded,
                total,
                speed,
                ..
            } => publish(|state| {
                state.downloaded = downloaded;
                state.total = total;
                state.speed = speed;
            }),
            DownloadEvent::Unpacking => publish(|state| state.phase = DownloadPhase::Unpacking),
            DownloadEvent::Done(_) => {}
        })
        .await;
        match outcome {
            Ok(_) => publish(|state| state.phase = DownloadPhase::Done),
            Err(error) => {
                log::warn!("下载 mihomo 失败：{error:#}");
                remove_partial(&asset);
                publish(|state| state.phase = DownloadPhase::Failed(format!("{error:#}").into()));
            }
        }
    });
    *TASK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);
}

/// 取消正在进行的下载，清掉残留的临时文件，状态回到空闲。
/// 已经进了解压那一步就让它做完（见 [`DownloadPhase::Unpacking`]）。
pub fn cancel() {
    if state().phase == DownloadPhase::Unpacking {
        return;
    }
    if let Some(handle) = TASK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        handle.abort();
    }
    let asset = state().asset.to_string();
    remove_partial(&asset);
    publish(|state| *state = DownloadState::default());
}

/// 把一次没下完的临时压缩包清掉。尽力而为。
fn remove_partial(asset: &str) {
    if asset.is_empty() {
        return;
    }
    if let Ok(dir) = default_binary_dir() {
        let _ = std::fs::remove_file(temp_archive_path(&dir, asset));
    }
}

/// 字节数 → 一句短话（`"12.3 MB"`）。
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.0} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
#[path = "../tests/clash/downloader.rs"]
mod tests;
