//! 按平台安装已下载、已验签的更新包。
//!
//! | 平台 | 安装形态 | 做法 |
//! | --- | --- | --- |
//! | Windows | Inno Setup 安装版 | 静默运行 setup.exe，装完由安装包重新拉起应用 |
//! | Windows | 免安装（tar.gz） | 不自动装，打开下载页 |
//! | macOS | `.app`（tar.gz 里的） | 解包、替换正在运行的 bundle、重启 |
//! | Linux | deb / rpm | `pkexec` 或 `sudo -S` 跑发行版的包管理器 |
//! | Linux | 不是包管理器装的 | 不自动装，打开下载页 |
//!
//! "要不要提权、怎么提"全在这一层；下载与校验在 [`super::download`] 里，
//! 永远以用户身份跑。

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};

/// 安装计划：用户点"立即更新"之前，界面要把这些摆给用户看。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallPlan {
    /// 将执行的安装方式。
    pub method: InstallMethod,
}

/// 安装方式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallMethod {
    /// 静默运行 Windows 安装包。
    WindowsInstaller,
    /// 用包管理器安装：`deb` 或 `rpm`。
    LinuxPackage { format: PackageFormat },
    /// 替换 `.app` bundle 并重启。
    MacAppBundle,
    /// 不自动安装，只打开下载页。
    ManualDownload,
}

/// Linux 包格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageFormat {
    Deb,
    Rpm,
}

/// 安装结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallResult {
    /// 安装程序已经拉起，应用应当立刻退出，让安装包替换文件。
    RestartRequired,
    /// 已经装好，需要用户手动重启应用。
    InstalledRestartManually,
    /// 做不到自动安装，用户要去下载页。括号里是具体原因，界面查表翻成文案；
    /// 后端 crate 不拼用户可见的句子（与"各后端不认识界面类型"同一条规矩）。
    Manual(ManualReason),
}

/// 为什么不能自动安装。界面按这一档查 `update.manual.*` 文案。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualReason {
    /// Windows 免安装版（tar.gz 解出来的，没有 Inno 的卸载器）。
    PortableBuild,
    /// Linux 上认不出包管理器，或者程序不是包管理器装的。
    NotPackageManaged,
    /// macOS 不在 `.app` 里跑（比如 `cargo run` 出来的裸二进制）。
    NotAppBundle,
    /// macOS 的 bundle 目录不可写。
    BundleNotWritable,
    /// 提权时用户取消了认证。
    AuthCancelled,
    /// 这个版本没有签名（或客户端还没内置公钥），不能自动安装。
    NotSigned,
}

/// 能不能自动安装，以及怎么装。`asset_kind` 来自更新接口（`installer` /
/// `portable` / `archive` / `deb` / `rpm`），`package` 是已验签的本地文件。
///
/// 这个函数只做判断、不执行任何东西，界面可以放心地在用户点"立即更新"
/// 之前先调它，把结果摆给用户看。
pub fn plan_for_current_platform(asset_kind: &str, package: &Path) -> InstallPlan {
    let _ = package;
    match asset_kind {
        #[cfg(windows)]
        "installer" => InstallPlan {
            method: InstallMethod::WindowsInstaller,
        },
        #[cfg(windows)]
        "portable" => InstallPlan {
            method: InstallMethod::ManualDownload,
        },
        #[cfg(target_os = "macos")]
        "archive" => InstallPlan {
            method: InstallMethod::MacAppBundle,
        },
        #[cfg(target_os = "linux")]
        "deb" | "rpm" if is_package_managed() => {
            let format = if asset_kind == "deb" {
                PackageFormat::Deb
            } else {
                PackageFormat::Rpm
            };
            InstallPlan {
                method: InstallMethod::LinuxPackage { format },
            }
        }
        _ => InstallPlan {
            method: InstallMethod::ManualDownload,
        },
    }
}

/// [`download`] 发现没法自动安装时用的标记：调用方按它识别成
/// [`ManualReason::NotSigned`]，而不是一条普通下载错误。
pub const NO_SIGNATURE: &str = "update has no signature";

/// 产物种类说不清为什么不装时，落到这一档原因上。
fn manual_reason(asset_kind: &str) -> ManualReason {
    match asset_kind {
        "portable" => ManualReason::PortableBuild,
        "deb" | "rpm" => ManualReason::NotPackageManaged,
        "dmg" | "archive" if cfg!(target_os = "macos") => ManualReason::NotAppBundle,
        _ => ManualReason::PortableBuild,
    }
}

/// 执行安装。**调用前必须已经验签**（[`super::download`] 保证）。
pub fn install(asset_kind: &str, package: &Path) -> Result<InstallResult> {
    let plan = plan_for_current_platform(asset_kind, package);
    match plan.method {
        InstallMethod::WindowsInstaller => {
            #[cfg(windows)]
            {
                install_windows_setup(package)
            }
            #[cfg(not(windows))]
            {
                let _ = package;
                bail!("Windows 安装包只能在 Windows 上安装")
            }
        }
        InstallMethod::LinuxPackage { format } => {
            #[cfg(target_os = "linux")]
            {
                install_linux_package(package, format)
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (package, format);
                bail!("Linux 包只能在 Linux 上安装")
            }
        }
        InstallMethod::MacAppBundle => {
            #[cfg(target_os = "macos")]
            {
                install_macos_bundle(package)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = package;
                bail!("macOS 更新包只能在 macOS 上安装")
            }
        }
        InstallMethod::ManualDownload => Ok(InstallResult::Manual(manual_reason(asset_kind))),
    }
}

// ── Windows ──────────────────────────────────────────────────────────────

/// 免安装版认法：exe 旁边有没有 Inno Setup 留下的卸载器。安装版一定有
/// `unins000.exe`，免安装的 tar.gz 里没有。
#[cfg(windows)]
pub fn is_installed_build() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("unins000.exe")))
        .is_some_and(|uninstaller| uninstaller.is_file())
}

#[cfg(not(windows))]
pub fn is_installed_build() -> bool {
    true
}

#[cfg(windows)]
fn install_windows_setup(package: &Path) -> Result<InstallResult> {
    // 只**启动**安装包，不等它跑完：安装器要替换的文件里就有当前 exe，
    // 一边等它成功、一边自己还占着那些文件，Inno 只能报"文件在使用中"。
    // 拉起子进程后调用方立刻退出应用，交给安装器收尾；装完 `[Run]` 那条
    // （见 scripts/win-installer.iss）会把新版本重新拉起来。
    //
    // /SILENT 显示进度条但不提问；/SUPPRESSMSGBOXES 把剩余对话框也压掉；
    // /NORESTART 不让安装器自己重启机器；/CLOSEAPPLICATIONS 允许它在静默
    // 模式下直接关掉仍占着文件的进程，而不弹"请关闭以下程序"。
    Command::new(package)
        .args([
            "/SILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/CLOSEAPPLICATIONS",
        ])
        .spawn()
        .context("启动 Windows 安装包失败")?;
    Ok(InstallResult::RestartRequired)
}

// ── Linux ────────────────────────────────────────────────────────────────

/// 本机有没有包管理器（deb 系有 `dpkg`，rpm 系有 `rpm`）。两者都没有时
/// 只提示下载，不硬装。
#[cfg(target_os = "linux")]
pub fn is_package_managed() -> bool {
    which("dpkg").is_some() || which("rpm").is_some()
}

/// `which` 的极简版：按 PATH 逐段找可执行文件。不引新依赖。
#[cfg(target_os = "linux")]
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// `/usr/bin` 里的 winssh-pro 是不是由包管理器装的。用户自己解 tar.gz 装到
/// `/usr/local/bin` 或家目录时认不出来，就不自动装 —— 否则会把文件装到不受
/// 管的位置，下次更新状态就乱了。
#[cfg(target_os = "linux")]
pub fn binary_is_package_managed() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| PathBuf::from("/usr/bin/winssh-pro"));
    let probe = if which("dpkg").is_some() { "dpkg" } else { "rpm" };
    let args: &[&str] = if probe == "dpkg" { &["-S"] } else { &["-qf"] };
    Command::new(probe)
        .args(args)
        .arg(&exe)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn install_linux_package(package: &Path, format: PackageFormat) -> Result<InstallResult> {
    // 命令全部是 argv 数组、不经 shell；包路径只来自我们自己写死的缓存目录。
    let (program, args): (&str, Vec<String>) = match format {
        PackageFormat::Deb => (
            "apt-get",
            vec![
                "install".into(),
                "-y".into(),
                "--no-install-recommends".into(),
                // 另一个 apt 正在跑时等一会儿，而不是立刻报"锁被占"。
                "-o".into(),
                "DPkg::Lock::Timeout=120".into(),
                package.display().to_string(),
            ],
        ),
        PackageFormat::Rpm => (
            "dnf",
            vec![
                "install".into(),
                "-y".into(),
                package.display().to_string(),
            ],
        ),
    };

    // 桌面有 polkit 就走 pkexec：认证框由系统弹，应用不碰密码。
    // 没有 pkexec（或没有认证 agent）才退回 sudo，密码从 stdin 喂进去 ——
    // 绝不放进 argv，`ps` 里看不到。
    let mut command = if which("pkexec").is_some() {
        let mut command = Command::new("pkexec");
        command.arg("env").arg("DEBIAN_FRONTEND=noninteractive");
        command.arg(program);
        command
    } else if which("sudo").is_some() {
        let mut command = Command::new("sudo");
        command.arg("-S").arg("-n").arg("--").arg(program);
        command
    } else {
        return Ok(InstallResult::Manual(ManualReason::NotPackageManaged));
    };
    command.args(&args);

    let output = command
        .output()
        .with_context(|| format!("启动 {program} 失败"))?;
    if output.status.success() {
        return Ok(InstallResult::InstalledRestartManually);
    }

    // 退出码要分开：认证被取消不是"安装失败"，不该糊用户一脸错误。
    let code = output.status.code();
    let stderr = String::from_utf8_lossy(&output.stderr);
    match code {
        // pkexec：126 = 用户取消认证，127 = 未授权。
        Some(126) | Some(127) if program_is_pkexec(&command) => {
            Ok(InstallResult::Manual(ManualReason::AuthCancelled))
        }
        _ => bail!("{program} 返回失败（{output:?}）：{}", stderr.trim()),
    }
}

#[cfg(target_os = "linux")]
fn program_is_pkexec(command: &Command) -> bool {
    Path::new(command.get_program())
        .file_name()
        .is_some_and(|name| name == "pkexec")
}

// ── macOS ────────────────────────────────────────────────────────────────

/// 从 tar.gz 里解出 `WinSSH Pro.app`，替换当前运行的那一份。
///
/// 应用自己下载的文件不会被打上 quarantine 标记，替换后 Gatekeeper 不拦。
/// 当前 bundle 不可写（`/Applications` 权限、App Translocation 的只读临时
/// 路径）时退回到"手动下载"。
#[cfg(target_os = "macos")]
fn install_macos_bundle(package: &Path) -> Result<InstallResult> {
    let current = std::env::current_exe().context("找不到当前可执行文件")?;
    // .../WinSSH Pro.app/Contents/MacOS/winssh-pro → .../WinSSH Pro.app
    let Some(bundle) = current
        .ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
    else {
        return Ok(InstallResult::Manual(ManualReason::NotAppBundle));
    };
    if !is_writable(bundle) {
        return Ok(InstallResult::Manual(ManualReason::BundleNotWritable));
    }

    let staging = std::env::temp_dir().join(format!("winssh-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).context("创建解包目录失败")?;

    let unpack = || -> Result<PathBuf> {
        let file = std::fs::File::open(package).context("打开更新包失败")?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(&staging).context("解包更新包失败")?;
        let extracted = staging
            .read_dir()?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "app"))
            .context("更新包里没有 .app")?;
        Ok(extracted)
    };
    let extracted = unpack();
    let extracted = match extracted {
        Ok(path) => path,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };

    // 原地替换：先把旧 bundle 挪开，再把新的搬进来。两步之间极短，且应用
    // 马上就要退出重启；真的在多步之间失败，旧 bundle 还在旁边的 .old 里。
    let backup = bundle.with_extension("app.old");
    let _ = std::fs::remove_dir_all(&backup);
    std::fs::rename(bundle, &backup).context("移开旧的应用包失败")?;
    if let Err(error) = std::fs::rename(&extracted, bundle) {
        // 失败就把旧的挪回去，别让用户失去应用。
        let _ = std::fs::rename(&backup, bundle);
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error).context("替换应用包失败");
    }
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_dir_all(&staging);

    // 重新拉起新版本；旧进程马上退出。
    Command::new("open")
        .arg("-n")
        .arg(bundle)
        .status()
        .context("重新打开应用失败")?;
    Ok(InstallResult::RestartRequired)
}

#[cfg(target_os = "macos")]
fn is_writable(path: &Path) -> bool {
    let probe = path.join(".winssh-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
#[path = "tests/updater/install.rs"]
mod tests;
