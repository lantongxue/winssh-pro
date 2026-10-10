//! 传输队列的模型：一个文件从本地到远端（或反过来）的进度状态。
//!
//! 这一层不含 I/O —— 真正搬字节的是 [`crate::service::SftpService::spawn_upload`]
//! 之类的后台任务，它把进度发回来，界面把最新一份存进这里的 [`TransferItem`]。
//! 之所以在这里算速率而不是在后台算：进度事件本来就是"某个时刻已搬了多少字节"，
//! 速率是它的差分，属于展示口径（和 [`crate::monitor`] 里两次读数相减同一个道理）。

use std::time::{Duration, SystemTime};

use gpui_kit::SharedString;

use super::entry::human_size;
use rust_i18n::t;

/// 速率的发布间隔。块级事件一秒能来几千个，每个都刷数字只会闪成一片；
/// 一秒一跳的平均速率读得清，也足够看出"在动"。
const SPEED_UPDATE_INTERVAL: Duration = Duration::from_secs(1);

/// "这次是用户自己撤的"这件事在层与层之间靠这个记号传。
///
/// 搬字节那一层报错只有一个 `String` 能带回来（[`TransferEvent::Done`] 的
/// `error`），而界面要能把"撤了"和"真失败了"分开：撤了不弹失败通知，状态落成
/// [`TransferStatus::Canceled`]。所以约一个记号，两边比它。
///
/// **不给用户看** —— 它是个内部标记，不是文案。要显示时用
/// [`TransferStatus::label`]（那一条才查语言表）。以前这里比的是中文的
/// "已取消"，那样一翻译就静默失效：撤一次传输会弹一条"失败"。
pub const CANCELED: &str = "sftp:canceled";

/// 方向决定图标与文案，也决定哪一端是源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferDirection {
    Upload,
    Download,
}

/// 一次传输的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferStatus {
    Queued,
    Transferring,
    /// 用户按了暂停：块与块之间停住，句柄还在，恢复接着搬。
    Paused,
    Completed,
    Failed,
    /// 用户按了取消，或者面板自己关掉时留下的。
    Canceled,
}

impl TransferStatus {
    /// 队列里"还没完事"的状态 —— 界面的脉冲提示与自动清理都按这个判断。
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Transferring | Self::Paused)
    }

    /// 抽屉里那个状态小药丸的文字。
    pub fn label(self) -> SharedString {
        match self {
            Self::Queued => t!("transfer.status_queued"),
            Self::Transferring => t!("transfer.status_transferring"),
            Self::Paused => t!("transfer.status_paused"),
            Self::Completed => t!("transfer.status_completed"),
            Self::Failed => t!("transfer.status_failed"),
            Self::Canceled => t!("transfer.status_canceled"),
        }
        .into()
    }
}

/// 队列里的一行。
#[derive(Clone, Debug)]
pub struct TransferItem {
    pub id: u64,
    pub filename: SharedString,
    pub direction: TransferDirection,
    /// 总字节。大小未知（对端没给）时为 0，此时进度条只能靠状态说明走到哪了。
    pub total: u64,
    pub transferred: u64,
    pub status: TransferStatus,
    /// 失败原因，来自对端或本地文件系统，原样展示。
    pub error: Option<SharedString>,
    /// 上一次收到进度更新的时刻，速率差分的窗口尽头。
    pub last_update: Option<SystemTime>,
    /// 上一次算出的速率，字节/秒。第一个事件之前没有速率。
    pub speed_bps: Option<f64>,
    /// 上一次**发布**速率的时刻。速率一秒才发布一次，它与 [`Self::last_update`]
    /// 是两回事：差分每个事件都做，发布要攒满一个间隔。
    pub last_speed_at: Option<SystemTime>,
    /// 自上次发布以来攒下的字节，到点折成这一段的平均速率。
    pub speed_window_bytes: f64,
    /// 目录传输摊平出来的子项，顺序就是任务里逐个搬运的顺序。
    /// 单文件传输为空，那一行没有展开箭头。
    pub children: Vec<TransferChild>,
}

impl TransferItem {
    /// 0.0 ~ 1.0。总数未知时不猜，给 0（进度条靠状态色表示"在跑"）。
    pub fn fraction(&self) -> f32 {
        fraction_of(self.total, self.transferred, self.status)
    }

    /// 是不是目录传输：有子项的才有展开一说。
    pub fn is_folder(&self) -> bool {
        !self.children.is_empty()
    }

    /// 主行落定后同步子行：还没走到终点的跟着主行走同一个终点。
    /// 成功时把字节数补齐（漏收的事件不再补报）；取消与失败只改状态，
    /// 半截就是半截，不虚报字节数。
    pub fn settle_children(&mut self, status: TransferStatus) {
        for child in &mut self.children {
            if child.status.is_active() {
                child.status = status;
                if status == TransferStatus::Completed {
                    child.transferred = child.transferred.max(child.total);
                }
            }
        }
    }

    /// 百分比，给 [`gpui_kit::component::progress::Progress`]（它要 0~100）。
    pub fn progress(&self) -> f32 {
        self.fraction() * 100.0
    }

    /// 速率文案，单位与原型的 `14.8 MB/s` 一致（1024 进制）。
    ///
    /// 完成后仍然显示最后一段的速率（原型也是这么留着的）；从没动过就是 `—`。
    pub fn speed_label(&self) -> String {
        match self.speed_bps {
            Some(speed) => format!("{}/s", human_size(speed as u64)),
            None => "—".to_owned(),
        }
    }

    /// 收下一个进度事件：更新已搬字节与速率，必要时把状态推到"传输中"。
    ///
    /// 速率按"距上次更新多久搬了多少"差分，但**不每个事件都发布** —— 32 KiB
    /// 一块、快的时候一秒几千块，逐块刷新只会闪成一片。这里把没发布的字节
    /// 攒起来，满 [`SPEED_UPDATE_INTERVAL`] 才折成这一段的平均速率发出去；
    /// 完成与失败不再算速率，留下的那个数字就是最后发布的。
    pub fn record_progress(&mut self, transferred: u64, at: SystemTime) {
        if transferred <= self.transferred && self.transferred > 0 {
            return;
        }
        let delta_bytes = transferred.saturating_sub(self.transferred) as f64;
        let delta_secs = match self.last_update {
            Some(previous) => at
                .duration_since(previous)
                .unwrap_or(Duration::ZERO)
                .as_secs_f64(),
            None => 0.0,
        };

        self.transferred = transferred.max(self.transferred);
        self.last_update = Some(at);
        if delta_secs > 0.0 {
            match self.last_speed_at {
                // 从没发布过：第一段差分直接当速率，别让数字空着。
                None => {
                    self.speed_bps = Some(delta_bytes / delta_secs);
                    self.last_speed_at = Some(at);
                }
                Some(published) => {
                    self.speed_window_bytes += delta_bytes;
                    let elapsed = at.duration_since(published).unwrap_or(Duration::ZERO);
                    if elapsed >= SPEED_UPDATE_INTERVAL {
                        self.speed_bps = Some(self.speed_window_bytes / elapsed.as_secs_f64());
                        self.last_speed_at = Some(at);
                        self.speed_window_bytes = 0.0;
                    }
                }
            }
        }
        if self.status == TransferStatus::Queued && transferred > 0 {
            self.status = TransferStatus::Transferring;
        }
    }

    /// 把速率观测重置干净：恢复暂停的传输时用 —— 暂停的时长不能摊进下一段
    /// 速率，攒着没发布的字节也该作废重数。
    pub fn reset_rate(&mut self) {
        self.last_update = None;
        self.last_speed_at = None;
        self.speed_window_bytes = 0.0;
    }
}

/// 目录传输里一个子文件自己的搬运状态，展开目录行时铺在下面。
///
/// 聚合进度（整棵树一条）仍归 [`TransferItem`]；子行既回答"这一个文件走到哪了"，
/// 也算自己那一份速率。目录并发后每个文件都在一个持续的窗口里搬（不再是几十毫秒
/// 一闪而过），逐子项差分有意义了 —— 口径与 [`TransferItem`] 一致，满一个发布窗口
/// 才刷一次，免得数字抖。
#[derive(Clone, Debug)]
pub struct TransferChild {
    /// 相对上传根目录的路径（`docs/img.png`），一行一个文件。
    pub filename: SharedString,
    /// 文件大小，任务规划时就数好了。
    pub total: u64,
    pub transferred: u64,
    pub status: TransferStatus,
    speed_bps: Option<f64>,
    last_update: Option<SystemTime>,
    last_speed_at: Option<SystemTime>,
    speed_window_bytes: f64,
}

impl TransferChild {
    /// 新排进目录的一个子文件：还没动过，速率空着。
    pub fn new(filename: SharedString, total: u64) -> Self {
        Self {
            filename,
            total,
            transferred: 0,
            status: TransferStatus::Queued,
            speed_bps: None,
            last_update: None,
            last_speed_at: None,
            speed_window_bytes: 0.0,
        }
    }

    /// 0.0 ~ 1.0，口径与 [`TransferItem::fraction`] 相同。
    pub fn fraction(&self) -> f32 {
        fraction_of(self.total, self.transferred, self.status)
    }

    /// 百分比，给 [`gpui_kit::component::progress::Progress`]（它要 0~100）。
    pub fn progress(&self) -> f32 {
        self.fraction() * 100.0
    }

    /// 速率文案，单位与 [`TransferItem::speed_label`] 一致（1024 进制）。从没
    /// 算出过速率就是 `—`。
    pub fn speed_label(&self) -> String {
        match self.speed_bps {
            Some(speed) => format!("{}/s", human_size(speed as u64)),
            None => "—".to_owned(),
        }
    }

    /// 收下一个子项进度事件：`transferred` 是这一个文件自己已搬的字节。
    /// 更新字节数并按 [`SPEED_UPDATE_INTERVAL`] 窗口折算这一份文件的速率。
    pub fn record_progress(&mut self, transferred: u64, at: SystemTime) {
        if transferred <= self.transferred && self.transferred > 0 {
            return;
        }
        let delta_bytes = transferred.saturating_sub(self.transferred) as f64;
        let delta_secs = match self.last_update {
            Some(previous) => at
                .duration_since(previous)
                .unwrap_or(Duration::ZERO)
                .as_secs_f64(),
            None => 0.0,
        };

        self.transferred = self.transferred.max(transferred);
        self.last_update = Some(at);
        if delta_secs > 0.0 {
            match self.last_speed_at {
                None => {
                    self.speed_bps = Some(delta_bytes / delta_secs);
                    self.last_speed_at = Some(at);
                }
                Some(published) => {
                    self.speed_window_bytes += delta_bytes;
                    let elapsed = at.duration_since(published).unwrap_or(Duration::ZERO);
                    if elapsed >= SPEED_UPDATE_INTERVAL {
                        self.speed_bps = Some(self.speed_window_bytes / elapsed.as_secs_f64());
                        self.last_speed_at = Some(at);
                        self.speed_window_bytes = 0.0;
                    }
                }
            }
        }
        if self.status == TransferStatus::Queued && transferred > 0 {
            self.status = TransferStatus::Transferring;
        }
    }

    /// 把速率观测重置干净：整条目录传输暂停后恢复时用 —— 暂停的时长不能摊进
    /// 某个子文件下一段的速率里。
    pub fn reset_rate(&mut self) {
        self.last_update = None;
        self.last_speed_at = None;
        self.speed_window_bytes = 0.0;
    }

    /// 这个文件落定了（改名成功）：字节数补齐，状态落成"已完成"。
    pub fn mark_done(&mut self) {
        self.transferred = self.transferred.max(self.total);
        self.status = TransferStatus::Completed;
    }
}

/// 比例的统一口径：总数未知（0）时不猜，只有完成才给满格。
fn fraction_of(total: u64, transferred: u64, status: TransferStatus) -> f32 {
    if total == 0 {
        return if status == TransferStatus::Completed {
            1.0
        } else {
            0.0
        };
    }
    (transferred as f64 / total as f64).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
#[path = "../tests/sftp/transfer.rs"]
mod tests;
