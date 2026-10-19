//! 自动更新服务：检查更新、按平台安装。
//!
//! 界面只认这一层，`winssh_updater` 的具体形状（HTTP、签名、安装命令）留在
//! 那个后端 crate 里。检查更新本身是网络操作，异步做；安装里的提权步骤是
//! 同步阻塞的，由界面放到后台线程里调 —— 这一层不替调用方决定线程模型。

use anyhow::Result;

use crate::updater::{self, AvailableUpdate};

/// 自动更新的业务入口。
pub struct UpdateService;

impl UpdateService {
    /// 检查有没有新版本。`current` 用 `env!("CARGO_PKG_VERSION")`。
    ///
    /// 预发布构建（`-dev`）直接返回 `None`，不打扰开发者。
    ///
    /// 网络请求经 [`crate::db::run`] 桥到应用级 tokio runtime 上跑：reqwest
    /// 建客户端与发请求都要求有 tokio reactor，而这个方法是从 gpui 的
    /// executor（`cx.spawn` / `window.spawn`）里调进来的 —— 在 gpui 的
    /// executor 上直接 `.await` 它，启动就会 panic
    /// "there is no reactor running, must be called from the context of a
    /// Tokio 1.x runtime"。`run` 返回的只是一个取结果的信封，在任何
    /// executor 上 await 都行，这是数据层与 gpui 之间定的界河。
    pub async fn check(&self, current: &str) -> Result<Option<AvailableUpdate>> {
        let current = current.to_owned();
        crate::db::run(async move { updater::check(&current).await }).await
    }

    /// 当前版本能不能自动检查（是不是正式版）。
    pub fn checkable(current: &str) -> bool {
        updater::Version::parse(current).is_some_and(|version| !version.is_prerelease())
    }

    /// 安装已经下载好的更新包。下载与校验由 `updater::state::begin` 完成，
    /// 这里只负责安装。
    ///
    /// 同步阻塞（提权、跑安装器）：调用方应当放在后台线程
    /// （`cx.background_spawn`）里，不要占着界面线程。
    pub fn install_update(
        asset_kind: &str,
        package: &std::path::Path,
    ) -> Result<updater::InstallResult> {
        updater::install(asset_kind, package)
    }
}
