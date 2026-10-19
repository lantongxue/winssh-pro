//! 自动更新：检查、下载、校验、安装。
//!
//! ## 一次更新的完整路程
//!
//! 1. [`check`] 请求 `https://winssh.net/api/v1/update`（云函数实现见
//!    winssh.net 仓库的 `cloud-functions/update.go`），拿到新版本号、发版日志
//!    （就是 GitHub Release 的正文，由 git-cliff 生成）与目标平台的产物清单。
//!    版本比较、该给哪个文件都由服务端决定，客户端只负责按形状解析。
//! 2. [`download`] 把产物流式下到用户数据目录 `winssh-pro/updates/<版本>/`，
//!    期间先比 sha256、再验 minisign 签名（见 [`signature`]）。**验不过就不
//!    往下走** —— 自动更新的信任根是编译进二进制的那把公钥，不是 HTTPS。
//! 3. [`install`] 按平台安装：
//!    - Windows：静默跑 Inno Setup 安装包，装完自己重新拉起应用；
//!    - macOS：解包出 `.app`，替换正在运行的 bundle 后重启；
//!    - Linux：`pkexec`（或 `sudo -S`）跑发行版的包管理器装上 deb/rpm。
//!
//! 安装形态认不出来时（Windows 免安装版、不是包管理器装的 Linux、没有写
//! 权限的 macOS）不硬装，只把用户引到 release 页面 —— 见 [`InstallPlan`]。
//!
//! ## 进度怎么走
//!
//! 与 mihomo 下载器同一套：进程级的 `watch` 通道（[`state`] / [`subscribe`]），
//! 界面订阅，弹窗关掉也不打断下载。下载任务跑在应用级 runtime 上。

pub mod check;
pub mod download;
pub mod install;
pub mod semver;
pub mod signature;
pub mod state;

pub use check::{AvailableUpdate, ReleaseAsset, check};
pub use download::{DownloadEvent, download};
pub use install::{InstallPlan, InstallResult, ManualReason, install, plan_for_current_platform};
pub use semver::{Version, compare};
pub use state::{Phase, State, begin, cancel, subscribe};

/// 字节数的短写法，进度条与下载说明共用（KB / MB）。
///
/// 与 mihomo 下载器里那份同一个算法，但不跨 crate 复用：那个是下载 UI 的
/// 私有小工具，为它开一条公共依赖不值得；两行代码的重复好过一层耦合。
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{} KB", (bytes as f64 / 1024.0).round().max(1.0) as u64)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
