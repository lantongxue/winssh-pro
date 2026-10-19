//! 下载进度与状态。界面订阅 [`subscribe`]，弹窗关掉也不打断下载。
//!
//! 形状照 mihomo 下载器（`winssh_clash::downloader`）：进程级一份
//! `watch` 通道，谁想看谁订阅，订阅时先拿到此刻那一份。状态里只放界面要画
//! 的东西 —— 版本号、阶段、已下载字节数与速度。

use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::watch;

use super::check::AvailableUpdate;
use super::download::DownloadEvent;

/// 一次更新的阶段。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// 什么都没做。
    #[default]
    Idle,
    /// 正在下载。
    Downloading,
    /// 下载完了，正在校验 sha256 与签名。
    Verifying,
    /// 已就绪，等用户点"重启并安装"。
    Ready,
    /// 正在安装（提权、跑安装器）。
    Installing,
    /// 失败。里面是给用户看的原因。
    Failed(String),
    /// 不能自动安装，需要手动下载。`Phase` 与安装计划共用同一组原因。
    Manual(super::install::ManualReason),
}

/// 此刻的下载状态。
#[derive(Clone, Debug, Default)]
pub struct State {
    pub phase: Phase,
    /// 目标版本号。
    pub version: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// 平均速度，字节每秒。
    pub speed: u64,
    /// 已下载并校验通过的本地文件。
    pub package: Option<std::path::PathBuf>,
}

impl State {
    /// 已完成的比例 0.0 ~ 1.0。不知道总大小时是 `None`。
    pub fn fraction(&self) -> Option<f32> {
        match self.total {
            Some(total) if total > 0 => {
                Some((self.downloaded as f64 / total as f64).min(1.0) as f32)
            }
            _ => None,
        }
    }
}

static CHANGES: LazyLock<watch::Sender<Arc<State>>> =
    LazyLock::new(|| watch::channel(Arc::new(State::default())).0);

/// 在跑的那个下载任务。取消时 abort。
static TASK: LazyLock<Mutex<Option<tokio::task::JoinHandle<()>>>> =
    LazyLock::new(Default::default);

/// 此刻那一份。
pub fn state() -> Arc<State> {
    CHANGES.borrow().clone()
}

/// 订阅变化。拿到手先读一次值，再等 `changed()`。
pub fn subscribe() -> watch::Receiver<Arc<State>> {
    CHANGES.subscribe()
}

fn publish(edit: impl FnOnce(&mut State)) {
    CHANGES.send_modify(|current| {
        let mut next = State::clone(current);
        edit(&mut next);
        *current = Arc::new(next);
    });
}

/// 重置为初始状态。
pub fn reset() {
    publish(|state| *state = State::default());
}

/// 开始下载 `update`。已经在下了就什么也不做。
///
/// 任务跑在应用级 runtime 上：弹窗关掉不该把它连带掐断。下载完、校验完
/// 进入 [`Phase::Ready`]，安装由用户点按钮触发（见 [`super::install`]）。
pub fn begin(update: AvailableUpdate) {
    let phase = state().phase.clone();
    if !matches!(phase, Phase::Idle | Phase::Failed(_) | Phase::Ready) {
        return;
    }
    let version = update.version.clone();
    publish(|state| {
        *state = State {
            phase: Phase::Downloading,
            version: version.clone(),
            ..State::default()
        };
    });

    let handle = winssh_db::runtime().spawn(async move {
        let outcome = super::download::download(&update, |event| match event {
            DownloadEvent::Start { .. } => {}
            DownloadEvent::Progress {
                downloaded,
                total,
                speed,
            } => publish(|state| {
                state.downloaded = downloaded;
                state.total = total;
                state.speed = speed;
            }),
            DownloadEvent::Verifying => publish(|state| state.phase = Phase::Verifying),
            DownloadEvent::Done(_) => {}
        })
        .await;
        match outcome {
            Ok(path) => publish(|state| {
                state.phase = Phase::Ready;
                state.package = Some(path);
            }),
            Err(error) => {
                log::warn!("下载更新包失败：{error:#}");
                let reason = format!("{error:#}");
                publish(|state| {
                    state.phase = if reason.contains(super::install::NO_SIGNATURE) {
                        // 没有签名 / 没有公钥：标成"不能自动装"，界面据此
                        // 换成手动下载的文案，不要把用户引去重试。
                        Phase::Manual(super::install::ManualReason::NotSigned)
                    } else {
                        Phase::Failed(reason)
                    }
                });
            }
        }
    });
    *TASK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);
}

/// 取消正在进行的下载。已经进入校验或安装阶段就不管了。
pub fn cancel() {
    if matches!(state().phase, Phase::Verifying | Phase::Installing) {
        return;
    }
    if let Some(handle) = TASK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        handle.abort();
    }
    publish(|state| *state = State::default());
}

/// 把安装阶段标出来，界面据此禁用按钮。安装本身是同步阻塞的提权操作，
/// 由界面在后台线程里调 [`super::install`]。
pub fn mark_installing() {
    publish(|state| state.phase = Phase::Installing);
}

/// 安装失败时把原因写回状态。
pub fn mark_failed(error: impl Into<String>) {
    publish(|state| state.phase = Phase::Failed(error.into()));
}

/// 供调用方判断"下载任务是否还在"。
pub fn busy() -> bool {
    matches!(
        state().phase,
        Phase::Downloading | Phase::Verifying | Phase::Installing
    )
}

#[cfg(test)]
#[path = "tests/updater/state.rs"]
mod tests;
