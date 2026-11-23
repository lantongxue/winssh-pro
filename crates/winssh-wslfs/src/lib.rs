//! WSL 发行版的接入层：文件系统与一次性命令。
//!
//! 文件这一半把 `\\wsl.localhost\<发行版>` 这条共享包装成和 SFTP 同一套目录
//! 模型，供底部文件栏使用。为什么走共享而不是在发行版里起 sftp-server：
//! `openssh-sftp-server` 不是每个发行版都装，装没装还得先探一次；而共享是
//! WSL 自己提供的，发行版一装上就在。代价是 Windows 这侧的 `Metadata` 没有
//! POSIX mode 与 uid —— 那两列由 [`meta`] 里那次 `find -printf` 补回来，
//! 补不到就显示 `—`，不编。
//!
//! 路径一律用发行版里的 Linux 绝对路径（`/home/kali`）在上下游走，只有真正
//! 碰文件系统的那一刻才经 [`unc::to_windows`] 换成共享路径。所以界面、传输
//! 队列与 SFTP 那一档说同一种话。
//!
//! 命令那一半（[`exec`]）把一段 POSIX sh 脚本喂进发行版的 `/bin/sh` 跑一次，
//! 监控面板的采集脚本与 `kill` 走它 —— 与 SSH 那一档同形，只是通道换成了
//! `wsl.exe` 的 stdin/stdout 管道。
//!
//! 所有阻塞动作（`std::fs`、`wsl.exe` 子进程）都经 `blocking` 派到 blocking
//! 池上：共享第一次访问会顺手把发行版拉起来，慢的时候以秒计，不能堵在
//! gpui 的前台线程上。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context as _, Result, anyhow, bail};
use rust_i18n::t;
use winssh_sftp::{FileEntry, FileKind, path};

// 面向用户的错误文案（连不上共享、命令失败……）走多语言查表，`t!` 展开成本
// crate 的 `_rust_i18n_t!`；`../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR
// 指回仓库根。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进
// dep-info，只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。
const _: &str = include_str!("../../../locales/app.yml");

pub mod exec;
pub mod meta;
pub mod unc;

/// 一条 WSL 发行版的文件入口。廉价克隆（一个 `Arc`），可以直接搬进后台任务。
#[derive(Clone)]
pub struct WslFs {
    inner: Arc<Inner>,
}

struct Inner {
    distro: String,
    /// 这个发行版的共享根。`wsl.localhost` 与 `wsl$` 二选一，第一次真去
    /// 访问时探定；探测本身要碰文件系统（还会把发行版拉起来），所以放
    /// OnceLock 里只做一次。
    root: OnceLock<PathBuf>,
    /// Linux 侧的家目录，问一次存下来（`wsl.exe -e sh -c 'echo $HOME'`）。
    home: Mutex<Option<String>>,
}

impl WslFs {
    pub fn new(distro: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                distro: distro.into(),
                root: OnceLock::new(),
                home: Mutex::new(None),
            }),
        }
    }

    /// 发行版里的家目录。第一次要的时候问一次就存下来。
    ///
    /// 问不到（`wsl.exe` 跑不起来、发行版里没有 `sh`）时退回 `/`：共享能读
    /// 就说明文件系统本身是通的，落在根目录也比让整栏停在错误上好；用户
    /// 从 `/home` 自己走两步就到家了。真正的失败（连共享都打不开）由列目录
    /// 那一步报出来。
    pub async fn home(&self) -> Result<String> {
        if let Some(home) = self.cached_home() {
            return Ok(home);
        }
        let distro = self.inner.distro.clone();
        let home = blocking(move || home_blocking(&distro)).await?;
        if let Ok(mut cached) = self.inner.home.lock() {
            *cached = Some(home.clone());
        }
        Ok(home)
    }

    /// 把界面里敲的那一格路径变成一个绝对路径：展开 `~`，再按 POSIX 词法
    /// 折叠 `.`/`..`。不解析软链接 —— 共享那边读目录时自然会跟过去。
    pub async fn resolve(&self, raw: &str) -> String {
        let home = self.home().await.ok();
        let expanded = path::expand_tilde(raw.trim(), home.as_deref());
        path::normalize(&expanded)
    }

    /// 列一个目录。排序与过滤留给界面，这里只报"发行版里有什么"。
    pub async fn list(&self, dir: &str) -> Result<Vec<FileEntry>> {
        let dir = path::normalize(dir);
        let this = self.clone();
        let label = dir.clone();
        blocking(move || this.list_blocking(&dir))
            .await
            .map_err(|error| {
                failed(
                    t!("sftp_err.action_read_dir", path => label).to_string(),
                    error,
                )
            })
    }

    /// 存不存在。只有"找不到"算不存在，其余错误照旧冒泡 —— 没权限不等于没有。
    pub async fn exists(&self, target: &str) -> Result<bool> {
        let target = path::normalize(target);
        let this = self.clone();
        blocking(move || this.exists_blocking(&target))
            .await
            .map_err(|error| failed(t!("sftp_err.action_stat_path").to_string(), error))
    }

    /// 是不是目录；不存在是 `None`。
    pub async fn is_dir(&self, target: &str) -> Result<Option<bool>> {
        let target = path::normalize(target);
        let this = self.clone();
        let label = target.clone();
        blocking(move || this.is_dir_blocking(&target))
            .await
            .map_err(|error| {
                failed(
                    t!("sftp_err.action_stat_remote", path => label).to_string(),
                    error,
                )
            })
    }

    /// 这个文件的字节数。编辑器拿它判断"太大了，不开"。
    pub async fn file_len(&self, target: &str) -> Result<u64> {
        let target = path::normalize(target);
        let this = self.clone();
        let label = target.clone();
        blocking(move || this.file_len_blocking(&target))
            .await
            .map_err(|error| {
                failed(
                    t!("sftp_err.action_stat_remote", path => label).to_string(),
                    error,
                )
            })
    }

    /// 新建目录。已存在就报错（和 SFTP 那边一样，不悄悄并进去）。
    pub async fn create_dir(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        let this = self.clone();
        let label = target.clone();
        blocking(move || this.create_dir_blocking(&target))
            .await
            .map_err(|error| {
                failed(
                    t!("sftp_err.action_create_dir", path => label).to_string(),
                    error,
                )
            })
    }

    /// 上传建目录骨架时用：已经有就当是并进去。
    pub async fn create_dir_all(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        let this = self.clone();
        let label = target.clone();
        blocking(move || this.create_dir_all_blocking(&target))
            .await
            .map_err(|error| {
                failed(
                    t!("sftp_err.action_create_remote_dir", path => label).to_string(),
                    error,
                )
            })
    }

    /// 新建空文件。已存在就报错，不清空别人的东西。
    pub async fn create_file(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        let this = self.clone();
        let label = target.clone();
        blocking(move || this.create_file_blocking(&target))
            .await
            .map_err(|error| {
                failed(
                    t!("sftp_err.action_create_file", path => label).to_string(),
                    error,
                )
            })
    }

    /// 删一个条目。目录走递归；软链接只删链接本身，不顺着删到别处去。
    pub async fn remove(&self, entry: &FileEntry) -> Result<()> {
        let entry = entry.clone();
        let this = self.clone();
        let label = entry.path.to_string();
        blocking(move || this.remove_blocking(&entry))
            .await
            .map_err(|error| {
                failed(
                    t!("sftp_err.action_delete", path => label).to_string(),
                    error,
                )
            })
    }

    /// 重命名（给绝对路径时就是移动）。目标已存在就失败 —— 与 SFTP 那一版
    /// 协议的行为一致，要不要覆盖是调用方的事。
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (path::normalize(from), path::normalize(to));
        let this = self.clone();
        let (from_label, to_label) = (from.clone(), to.clone());
        blocking(move || this.rename_blocking(&from, &to))
            .await
            .map_err(|error| {
                failed(
                    t!(
                        "sftp_err.action_rename",
                        from => from_label,
                        to => to_label,
                    )
                    .to_string(),
                    error,
                )
            })
    }

    /// 改权限。`octal` 是 `0o644` 这种只含权限位的数。
    ///
    /// 共享这侧没有 chmod 的口子，所以回到发行版里跑一次 `chmod` —— 那是
    /// 真正拥有这个文件的系统，改的就是它自己的 mode。
    pub async fn set_mode(&self, target: &str, octal: u32) -> Result<()> {
        if octal > 0o7777 {
            bail!("{}", t!("sftp_err.bad_mode", mode => format!("{octal:o}")));
        }
        let target = path::normalize(target);
        let this = self.clone();
        let label = target.clone();
        blocking(move || this.set_mode_blocking(&target, octal))
            .await
            .map_err(|error| {
                failed(
                    t!("sftp_err.action_chmod", path => label).to_string(),
                    error,
                )
            })
    }

    /// 读一个文件的原始字节。文本编码由编辑器那一层决定，这里只搬字节。
    pub async fn read_bytes(&self, target: &str) -> Result<Vec<u8>> {
        let target = path::normalize(target);
        let this = self.clone();
        let label = target.clone();
        blocking(move || this.read_blocking(&target))
            .await
            .map_err(|error| {
                failed(
                    t!("sftp_err.action_read_file", path => label).to_string(),
                    error,
                )
            })
    }

    /// 整篇覆盖写回（字节已经由调用方按目标编码编好）。
    pub async fn write_bytes(&self, target: &str, bytes: Vec<u8>) -> Result<()> {
        let target = path::normalize(target);
        let this = self.clone();
        let label = target.clone();
        blocking(move || this.write_blocking(&target, &bytes))
            .await
            .map_err(|error| {
                failed(
                    t!("sftp_err.action_save_file", path => label).to_string(),
                    error,
                )
            })
    }

    /// 把共享根探定。首次调用会碰文件系统（还可能把发行版拉起来），所以从
    /// 异步任务里起的传输先 await 这一步，别让探测堵在 runtime 线程上；
    /// 探定之后 [`Self::windows_path`] 就只是拼字符串。
    pub async fn ensure_root(&self) -> Result<PathBuf> {
        let this = self.clone();
        blocking(move || this.root()).await
    }

    /// Linux 绝对路径 → Windows 共享路径。root 已探过就只是拼字符串；
    /// 第一次会碰一次文件系统，所以传输任务在 planning 阶段把它走热。
    pub fn windows_path(&self, linux: &str) -> Result<PathBuf> {
        let root = self.root()?;
        Ok(unc::to_windows(&root, &path::normalize(linux)))
    }

    // ── 共享根 ───────────────────────────────────────────────────────────

    fn cached_home(&self) -> Option<String> {
        self.inner.home.lock().ok().and_then(|home| home.clone())
    }

    /// 探共享根：先 `wsl.localhost`，再 `wsl$`。两个都读不出来才算失败。
    fn root(&self) -> Result<PathBuf> {
        if let Some(root) = self.inner.root.get() {
            return Ok(root.clone());
        }
        #[cfg(not(windows))]
        bail!("{}", t!("wslfs.err.unsupported"));
        #[cfg(windows)]
        {
            for candidate in unc::roots(&self.inner.distro) {
                let path = PathBuf::from(&candidate);
                if std::fs::metadata(&path).is_ok_and(|meta| meta.is_dir()) {
                    let _ = self.inner.root.set(path.clone());
                    return Ok(path);
                }
            }
            bail!(
                "{}",
                t!(
                    "wslfs.err.distro_not_found",
                    distro => self.inner.distro.clone(),
                )
            )
        }
    }

    // ── blocking 实现 ───────────────────────────────────────────────────

    fn list_blocking(&self, dir: &str) -> Result<Vec<FileEntry>> {
        let windows = self.windows_path(dir)?;
        let reader = std::fs::read_dir(&windows).with_context(|| windows.display().to_string())?;
        // Linux 侧的属性（权限、属主）是加分项，拿不到就退回 Windows 那半边。
        let mut linux = self.linux_attributes(dir);

        let mut entries = Vec::new();
        for item in reader.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            let Ok(fallback) = std::fs::symlink_metadata(item.path()) else {
                // 一个条目读不到属性（刚被删、悬空链接）就跳过它，别的照常列。
                continue;
            };
            let found = linux.remove(&name);
            let file_type = fallback.file_type();
            let fallback_kind = if file_type.is_symlink() {
                FileKind::Symlink
            } else if file_type.is_dir() {
                FileKind::Dir
            } else if file_type.is_file() {
                FileKind::File
            } else {
                FileKind::Special
            };
            entries.push(FileEntry {
                name: name.clone().into(),
                path: path::join(dir, &name).into(),
                kind: found.as_ref().map_or(fallback_kind, |entry| entry.kind),
                size: found.as_ref().map_or(fallback.len(), |entry| entry.size),
                mode: found.as_ref().and_then(|entry| entry.mode),
                uid: found.as_ref().and_then(|entry| entry.uid),
                owner: found
                    .as_ref()
                    .and_then(|entry| entry.owner.clone())
                    .map(Into::into),
                modified: found
                    .as_ref()
                    .and_then(|entry| entry.modified)
                    .or_else(|| fallback.modified().ok()),
            });
        }
        Ok(entries)
    }

    /// 在发行版里跑一次 `find`，把这一层条目的 mode/uid/属主拿回来。
    /// 失败是常态之一（精简发行版、`find` 的实现不认 `-printf`），记一条
    /// debug 日志就够了 —— 列目录本身不该因此失败。
    fn linux_attributes(&self, dir: &str) -> HashMap<String, meta::FoundEntry> {
        match self.find_blocking(dir) {
            Ok(stdout) => meta::parse(&stdout)
                .into_iter()
                .map(|entry| (entry.name.clone(), entry))
                .collect(),
            Err(error) => {
                log::debug!("WSL 目录属性查询失败，退回 Windows 侧属性：{error:#}");
                HashMap::new()
            }
        }
    }

    fn find_blocking(&self, dir: &str) -> Result<Vec<u8>> {
        // `read_dir` 那边相对路径是相对共享根的，`find` 这边也要说同一种话：
        // 补上开头的 `/`，免得被当成表达式里的选项。
        let absolute = match dir.starts_with('/') {
            true => dir.to_owned(),
            false => format!("/{dir}"),
        };
        let output = self.run_wsl(&[
            "-e",
            "find",
            &absolute,
            "-mindepth",
            "1",
            "-maxdepth",
            "1",
            "-printf",
            meta::FIND_FORMAT,
        ])?;
        // `find` 碰到一个读不到的子项就以非零退出，但已经印出来的记录仍是好的
        // —— 那种情况照收，只有"一个字都没有"才算这次查询失败。
        if !output.status.success() {
            if output.stdout.is_empty() {
                return Err(self.command_failed("find", &output));
            }
            log::debug!(
                "WSL 目录属性只取到一部分：{}",
                self.command_failed("find", &output)
            );
        }
        Ok(output.stdout)
    }

    fn exists_blocking(&self, target: &str) -> Result<bool> {
        let windows = self.windows_path(target)?;
        match std::fs::metadata(&windows) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn is_dir_blocking(&self, target: &str) -> Result<Option<bool>> {
        let windows = self.windows_path(target)?;
        match std::fs::metadata(&windows) {
            Ok(metadata) => Ok(Some(metadata.is_dir())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn file_len_blocking(&self, target: &str) -> Result<u64> {
        let windows = self.windows_path(target)?;
        Ok(std::fs::metadata(&windows)?.len())
    }

    fn create_dir_blocking(&self, target: &str) -> Result<()> {
        let windows = self.windows_path(target)?;
        std::fs::create_dir(&windows).with_context(|| windows.display().to_string())
    }

    fn create_dir_all_blocking(&self, target: &str) -> Result<()> {
        let windows = self.windows_path(target)?;
        std::fs::create_dir_all(&windows).with_context(|| windows.display().to_string())
    }

    fn create_file_blocking(&self, target: &str) -> Result<()> {
        let windows = self.windows_path(target)?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&windows)
        {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                bail!("{}", t!("sftp_err.already_exists", path => target))
            }
            Err(error) => Err(error).with_context(|| windows.display().to_string()),
        }
    }

    fn remove_blocking(&self, entry: &FileEntry) -> Result<()> {
        let windows = self.windows_path(&entry.path)?;
        let metadata =
            std::fs::symlink_metadata(&windows).with_context(|| windows.display().to_string())?;
        let file_type = metadata.file_type();
        // `entry.kind` 是 `find` 从发行版里读回来的，比 Windows 这侧对 reparse
        // point 的判断更可信：只要那边说是软链接，就只删链接本身，绝不递归。
        if entry.kind == FileKind::Symlink || file_type.is_symlink() {
            // Windows 上指向目录的软链接要用 remove_dir 才删得掉；先按文件删，
            // 不行再按目录删。两条都只删链接本身。
            return match std::fs::remove_file(&windows) {
                Ok(()) => Ok(()),
                Err(_) => {
                    std::fs::remove_dir(&windows).with_context(|| windows.display().to_string())
                }
            };
        }
        if metadata.is_dir() {
            return std::fs::remove_dir_all(&windows)
                .with_context(|| windows.display().to_string());
        }
        std::fs::remove_file(&windows).with_context(|| windows.display().to_string())
    }

    fn rename_blocking(&self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (self.windows_path(from)?, self.windows_path(to)?);
        std::fs::rename(&from, &to)
            .with_context(|| format!("{} → {}", from.display(), to.display()))
    }

    fn set_mode_blocking(&self, target: &str, octal: u32) -> Result<()> {
        let mode = format!("{octal:o}");
        let output = self.run_wsl(&["-e", "chmod", &mode, "--", target])?;
        if !output.status.success() {
            return Err(self.command_failed("chmod", &output));
        }
        Ok(())
    }

    fn read_blocking(&self, target: &str) -> Result<Vec<u8>> {
        let windows = self.windows_path(target)?;
        std::fs::read(&windows).with_context(|| windows.display().to_string())
    }

    fn write_blocking(&self, target: &str, bytes: &[u8]) -> Result<()> {
        let windows = self.windows_path(target)?;
        std::fs::write(&windows, bytes).with_context(|| windows.display().to_string())
    }

    // ── wsl.exe ─────────────────────────────────────────────────────────

    /// 在发行版里跑一个程序。`args` 从 `-e` 开始（与手敲 `wsl.exe -d …`
    /// 的那一段一致），stdout/stderr 都收回来由调用方解释。
    fn run_wsl(&self, args: &[&str]) -> Result<std::process::Output> {
        let mut command = std::process::Command::new("wsl.exe");
        command.arg("-d").arg(&self.inner.distro).args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // 与 terminal::wsl 的发行版探测同一处讲究：探个目录而已，
            // 别让控制台黑框闪一下。
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command.output().with_context(|| {
            t!(
                "wslfs.err.spawn",
                distro => self.inner.distro.clone(),
            )
            .to_string()
        })
    }

    fn command_failed(&self, command: &str, output: &std::process::Output) -> anyhow::Error {
        let stderr = decode_text(&output.stderr);
        let message = stderr.trim();
        let message = if message.is_empty() {
            format!("{:?}", output.status)
        } else {
            message.to_owned()
        };
        anyhow!(
            "{}",
            t!(
                "wslfs.err.command_failed",
                distro => self.inner.distro.clone(),
                command => command.to_owned(),
                message => message,
            )
        )
    }
}

/// 同步工作派到 blocking 池：共享访问与子进程都可能慢到以秒计。
async fn blocking<T, F>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    winssh_db::run(async move {
        tokio::task::spawn_blocking(work)
            .await
            .with_context(|| t!("wslfs.err.interrupted").to_string())?
    })
    .await
}

/// 问发行版要一次家目录。问不到给 `/`，理由见 [`WslFs::home`]。
fn home_blocking(distro: &str) -> Result<String> {
    let mut command = std::process::Command::new("wsl.exe");
    command.args(["-d", distro, "-e", "sh", "-c", "printf %s \"$HOME\""]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .with_context(|| t!("wslfs.err.spawn", distro => distro.to_owned()).to_string())?;
    if output.status.success() {
        let home = decode_text(&output.stdout).trim().to_owned();
        if home.starts_with('/') {
            return Ok(path::normalize(&home));
        }
    }
    log::warn!("WSL {distro} 的家目录问不出来，先从 / 开始");
    Ok("/".to_owned())
}

/// 把 `wsl.exe` 的管道输出还原成文本。
///
/// `wsl.exe` 自己的提示（代理配置、升级提醒）往 stderr 写的是 UTF-16LE，
/// 发行版里那个程序写的却是 UTF-8。判据与 `terminal::wsl` 一致：有 UTF-16LE
/// 的 BOM，或者 NUL 密度高，就按 UTF-16 解；否则按 UTF-8。
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    let bom = bytes.starts_with(&[0xFF, 0xFE]);
    let nuls = bytes.iter().filter(|byte| **byte == 0).count();
    if bom || nuls * 4 > bytes.len() {
        let skip = if bom { 2 } else { 0 };
        let units: Vec<u16> = bytes[skip..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// 把一次底层失败套进界面上那句"…失败：原因"的外壳。
fn failed(action: String, error: impl std::fmt::Display) -> anyhow::Error {
    anyhow!(
        "{}",
        t!(
            "sftp_err.failed",
            action => action,
            message => format!("{error:#}"),
            hint => "",
        )
    )
}

#[cfg(test)]
#[path = "tests/wslfs.rs"]
mod tests;
