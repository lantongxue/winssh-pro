//! 本地文件服务：底部文件面板左半栏（"Local Workstation"）的数据入口。
//!
//! 它存在的理由和别的 service 一样：界面不直接碰 `std::fs`。列目录虽然是本地
//! 操作，但慢盘与网络挂载（ NFS、SMB、 iCloud 的"未下载"）能让一次
//! `read_dir` 卡住几百毫秒，所以它派到 `spawn_blocking` 上去，前台一帧都不掉。
//!
//! 本地条目复用 [`FileEntry`]：两栏要画在同一套表头下，模型分两份只会让
//! 显示口径漂移。差别只在几个字段本地给不出（属主名），那就是 `None`。

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use gpui_kit::SharedString;

use crate::sftp::{FileEntry, FileKind};
use rust_i18n::t;

/// 本地这一侧的入口。没有状态，廉价克隆。
#[derive(Clone, Copy, Default)]
pub struct LocalFileService;

impl LocalFileService {
    /// 本地栏的起点：下载目录，没有就回家目录。
    ///
    /// 拿不到任何一个（容器里没有 `$HOME` 时会这样）就退回当前目录，
    /// 至少列出来的是一眼能认的东西。
    pub fn start_dir() -> PathBuf {
        dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// 一个目录里有什么。排序与过滤留给界面。
    pub async fn list(&self, dir: PathBuf) -> Result<Vec<FileEntry>> {
        crate::db::run(async move {
            tokio::task::spawn_blocking(move || list_blocking(&dir))
                .await
                .with_context(|| t!("local_file.list_interrupted").to_string())?
        })
        .await
    }

    /// 挑一个能写的下载目录：给的路径不是目录（或不存在）时退回上次的地方。
    /// 下载目标必须是目录，这一点由这里保证，而不是由界面猜。
    pub fn writable_dir(path: &Path) -> Result<PathBuf> {
        if path.is_dir() {
            return Ok(path.to_path_buf());
        }
        if path.as_os_str().is_empty() {
            return Ok(Self::start_dir());
        }
        Err(anyhow::anyhow!(
            "{}",
            t!("local_file.not_a_dir", path => path.display())
        ))
    }

    /// 用系统的默认程序打开一个本地条目（文件走关联程序，目录交给文件管理器）。
    ///
    /// 这一步交给操作系统，不解释内容：本地有什么扩展名、装了哪些程序，界面无从
    /// 预判，把打开的决定权还给系统才是诚实的做法。失败（没有关联程序、路径已不
    /// 存在）把系统原话带回来。
    pub fn open_with_default(path: &Path) -> Result<()> {
        let mut opener = Self::opener(path);
        let status = opener
            .status()
            .with_context(|| t!("local_file.open_failed", path => path.display()).to_string())?;
        // 各平台的 open/start/xdg-open 在"成功唤起"时都返回 0；非零就是它自己报了错。
        if !status.success() {
            anyhow::bail!(
                "{}",
                t!(
                    "local_file.open_rejected",
                    path => path.display(),
                    code => format!("{:?}", status.code()),
                )
            );
        }
        Ok(())
    }

    #[cfg(windows)]
    fn opener(path: &Path) -> std::process::Command {
        // `cmd /c start "" "路径"`：紧跟 start 的那个空串是窗口标题占位，缺了它 start
        // 会把带空格的路径当成标题。`CREATE_NO_WINDOW` 让那层 cmd 不闪一个黑框。
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/c", "start", ""])
            .arg(path)
            .creation_flags(CREATE_NO_WINDOW);
        cmd
    }

    #[cfg(target_os = "macos")]
    fn opener(path: &Path) -> std::process::Command {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(path);
        cmd
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn opener(path: &Path) -> std::process::Command {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(path);
        cmd
    }

    /// 删掉一个本地条目。目录连整棵子树一并删掉。
    ///
    /// 这一步直接交给操作系统的删除接口，而不是我们自己一行进制目录再删：目录走
    /// `std::fs::remove_dir_all`（Windows 上是系统那一套带 `\\?\` 长路径、逐层删除的
    /// 实现），文件与软链接走 `remove_file`（软链接删的是链接本身，不是它指向的东西）。
    /// 这一步不可逆，调用方（界面）负责先确认一次。
    pub async fn remove(path: PathBuf) -> Result<()> {
        crate::db::run(async move {
            tokio::task::spawn_blocking(move || remove_blocking(&path))
                .await
                .with_context(|| t!("local_file.delete_interrupted").to_string())?
        })
        .await
    }

    /// 读一个本地文本文件。主机表单从 `~/.ssh/` 里导入私钥走的就是这条。
    ///
    /// 不是 UTF-8 就报错而不是有损转换：调用方拿它去解私钥，一份被替换字符
    /// 蛀过的文本解出来只会是一句莫名其妙的"格式不认"。
    pub async fn read_text(path: PathBuf) -> Result<String> {
        crate::db::run(async move {
            tokio::task::spawn_blocking(move || {
                std::fs::read_to_string(&path).with_context(|| {
                    t!("local_file.read_failed", path => path.display()).to_string()
                })
            })
            .await
            .with_context(|| t!("local_file.read_interrupted").to_string())?
        })
        .await
    }

    /// 把一段文本写成一个本地文件，保险箱里"另存私钥为 .pem"走的就是这条。
    ///
    /// 类 Unix 系统上顺手把权限收成 `0600`：OpenSSH 见到一份组内或其他人读得到
    /// 的私钥会直接拒绝用它（`WARNING: UNPROTECTED PRIVATE KEY FILE!`），
    /// 那时候用户还得自己 `chmod` 一遍才明白发生了什么。Windows 上没有这一层，
    /// 文件跟着所在目录的 ACL 走。
    pub async fn write_text(path: PathBuf, contents: String) -> Result<()> {
        crate::db::run(async move {
            tokio::task::spawn_blocking(move || {
                std::fs::write(&path, contents.as_bytes()).with_context(|| {
                    t!("local_file.write_failed", path => path.display()).to_string()
                })?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    // 收紧权限失败不算写失败：文件已经躺在那儿了，报错反而会让
                    // 用户以为没写成而再存一次。日志留一行。
                    if let Err(error) =
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    {
                        log::warn!("私钥写好了，但没能把权限收成 0600：{error}");
                    }
                }
                Ok(())
            })
            .await
            .with_context(|| t!("local_file.write_interrupted").to_string())?
        })
        .await
    }
}

/// 真正的删除。同步的 `std::fs`，跑在 blocking 线程上（大目录、网络盘都可能慢）。
fn remove_blocking(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| t!("local_file.read_failed", path => path.display()).to_string())?;
    let result = if metadata.is_dir() {
        // 目录连里面的东西一并删掉：系统调用负责递归，不在这儿手写遍历。
        std::fs::remove_dir_all(path)
    } else {
        // 文件与软链接都走 remove_file：软链接删的是链接本身，不是它指向的东西。
        std::fs::remove_file(path)
    };
    result.with_context(|| t!("local_file.delete_failed", path => path.display()).to_string())
}

/// 真正的读取。同步的 `std::fs`，跑在 blocking 线程上。
fn list_blocking(dir: &Path) -> Result<Vec<FileEntry>> {
    let reader = std::fs::read_dir(dir)
        .with_context(|| t!("local_file.read_dir_failed", path => dir.display()).to_string())?;

    let mut entries = Vec::new();
    for item in reader.flatten() {
        let path = item.path();
        let Ok(metadata) = item.metadata() else {
            // 读不到一个条目的属性（悬空软链接、权限刚被改掉）就跳过它，
            // 但别的条目仍然要看得到 —— 一个坏名字不该让整栏空掉。
            continue;
        };
        entries.push(entry_of(path, &metadata));
    }
    Ok(entries)
}

fn entry_of(path: PathBuf, metadata: &std::fs::Metadata) -> FileEntry {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let file_type = metadata.file_type();

    FileEntry {
        name: SharedString::from(name),
        path: SharedString::from(path.to_string_lossy().into_owned()),
        kind: if file_type.is_dir() {
            FileKind::Dir
        } else if file_type.is_symlink() {
            FileKind::Symlink
        } else if file_type.is_file() {
            FileKind::File
        } else {
            FileKind::Special
        },
        size: metadata.len(),
        mode: mode_of(metadata),
        uid: uid_of(metadata),
        // 本地这一侧我们不查口令表（那要 `getpwuid`，还要缓存），
        // 所以属主只有数字 —— 和远端没给名字时同一个口径。
        owner: None,
        modified: metadata.modified().ok(),
    }
}

/// 权限位。Unix 直接给 st_mode；其他平台只有"只读/可读写"两档，
/// 那就照那两档说（`0o444` / `0o666`），不编一个看起来像 Unix 的完整模式。
#[cfg(unix)]
fn mode_of(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn mode_of(metadata: &std::fs::Metadata) -> Option<u32> {
    Some(if metadata.permissions().readonly() {
        0o444
    } else {
        0o666
    })
}

#[cfg(unix)]
fn uid_of(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;
    Some(metadata.uid())
}

#[cfg(not(unix))]
fn uid_of(_: &std::fs::Metadata) -> Option<u32> {
    None
}

/// 给一个下载目标挑最终落盘的名字：目录里已经有同名文件就加 `(2)`、`(3)`……
/// 直到没有为止。覆盖本地文件这种事，不该在没人要求的时候发生。
pub fn unique_local_path(dir: &Path, name: &str) -> PathBuf {
    unique_local_path_with(dir, name, |_| false)
}

/// 同 [`unique_local_path`]，另外让开 `taken` 认领过的名字：一批下载各自挑
/// 名字的那一刻都还没落盘，彼此在磁盘上看不见（RDP 剪贴板一次粘来几个顶层项
/// 就是这样）。
pub fn unique_local_path_with(dir: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let free = |path: &Path| !path.exists() && !taken(path);
    let first = dir.join(name);
    if free(&first) {
        return first;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => {
            (stem.to_owned(), Some(extension.to_owned()))
        }
        _ => (name.to_owned(), None),
    };
    for index in 2..1000 {
        let candidate = match &extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let path = dir.join(candidate);
        if free(&path) {
            return path;
        }
    }
    // 999 个同名还不让下载，那这个目录本身就有问题；把最后那个交给用户覆盖。
    dir.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_a_real_directory_with_real_numbers() {
        let root = std::env::temp_dir().join(format!("winssh-pro-local-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时目录");
        std::fs::write(root.join("notes.txt"), vec![b'x'; 1234]).expect("写文件");
        std::fs::create_dir(root.join("sub")).ok();

        let entries = crate::db::runtime()
            .block_on(LocalFileService.list(root.clone()))
            .expect("列本地目录");

        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"notes.txt"), "实际：{names:?}");
        let file = entries
            .iter()
            .find(|e| e.name == "notes.txt")
            .expect("有这一个");
        assert_eq!(file.size, 1234, "大小来自磁盘，不是估的");
        assert_eq!(file.kind, FileKind::File);
        assert!(file.modified.is_some(), "本地一定有 mtime");
        assert!(
            entries.iter().any(|e| e.kind == FileKind::Dir),
            "子目录也算目录"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_directory_reports_its_own_name() {
        let error = crate::db::runtime()
            .block_on(LocalFileService.list(PathBuf::from("/no/such/dir/winssh-pro")))
            .expect_err("不存在就该失败");
        let message = format!("{error:#}");
        assert!(
            message.contains("/no/such/dir/winssh-pro"),
            "要说清是哪个目录：{message}"
        );
    }

    #[test]
    fn downloads_never_overwrite_quietly() {
        let root = std::env::temp_dir().join(format!("winssh-pro-unique-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时目录");
        std::fs::write(root.join("app.tar.gz"), b"x").expect("写同名文件");
        std::fs::write(root.join("LICENSE"), b"x").expect("写无扩展名文件");
        std::fs::write(root.join(".hidden"), b"x").expect("写隐藏文件");

        assert_eq!(
            unique_local_path(&root, "other.txt"),
            root.join("other.txt"),
            "没有同名就用原名字"
        );
        assert_eq!(
            unique_local_path(&root, "app.tar.gz"),
            root.join("app.tar (2).gz"),
            "扩展名取最后一个点之后：与远端那套同一口径，不认 tar.gz"
        );
        assert_eq!(
            unique_local_path(&root, "LICENSE"),
            root.join("LICENSE (2)"),
            "没有扩展名也要有编号"
        );
        assert_eq!(
            unique_local_path(&root, ".hidden"),
            root.join(".hidden (2)"),
            "点开头的名字，那个点不是扩展名分隔符"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_download_target_must_be_a_directory() {
        let root = std::env::temp_dir().join(format!("winssh-pro-dir-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时目录");
        let file = root.join("afile");
        std::fs::write(&file, b"x").expect("写文件");

        assert_eq!(LocalFileService::writable_dir(&root).unwrap(), root);
        LocalFileService::writable_dir(&file).expect_err("文件不是目录");
        assert_eq!(
            LocalFileService::writable_dir(Path::new("")).unwrap(),
            LocalFileService::start_dir(),
            "空路径是还没选过，退回起点"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_start_directory_exists() {
        assert!(
            LocalFileService::start_dir().exists(),
            "本地栏一打开就该有东西可列：{}",
            LocalFileService::start_dir().display()
        );
    }

    #[test]
    fn removing_a_directory_takes_the_whole_subtree() {
        let root = std::env::temp_dir().join(format!("winssh-pro-rm-{}", std::process::id()));
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).expect("建多层目录");
        std::fs::write(nested.join("leaf.txt"), b"x").expect("写文件");

        crate::db::runtime()
            .block_on(LocalFileService::remove(root.clone()))
            .expect("递归删非空目录");
        assert!(!root.exists(), "整棵子树都该没了");
    }

    #[test]
    fn removing_a_file_leaves_its_siblings() {
        let root = std::env::temp_dir().join(format!("winssh-pro-rm1-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建目录");
        let victim = root.join("gone.txt");
        std::fs::write(&victim, b"x").expect("写文件");
        std::fs::write(root.join("keep.txt"), b"y").expect("写另一个");

        crate::db::runtime()
            .block_on(LocalFileService::remove(victim))
            .expect("删单个文件");
        assert!(!root.join("gone.txt").exists());
        assert!(root.join("keep.txt").exists(), "只删点名的那一个");
        std::fs::remove_dir_all(&root).ok();
    }
}
