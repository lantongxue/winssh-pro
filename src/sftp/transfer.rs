//! 传输队列的模型：一个文件从本地到远端（或反过来）的进度状态。
//!
//! 这一层不含 I/O —— 真正搬字节的是 [`crate::service::SftpService::spawn_upload`]
//! 之类的后台任务，它把进度发回来，界面把最新一份存进这里的 [`TransferItem`]。
//! 之所以在这里算速率而不是在后台算：进度事件本来就是"某个时刻已搬了多少字节"，
//! 速率是它的差分，属于展示口径（和 [`crate::monitor`] 里两次读数相减同一个道理）。

use std::time::{Duration, SystemTime};

use gpui::SharedString;

use super::entry::human_size;

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
    Completed,
    Failed,
    /// 用户按了取消，或者面板自己关掉时留下的。
    Canceled,
}

impl TransferStatus {
    /// 队列里"还没完事"的状态 —— 界面的脉冲提示与自动清理都按这个判断。
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Transferring)
    }

    /// 抽屉里那个状态小药丸的文字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "排队中",
            Self::Transferring => "传输中",
            Self::Completed => "已完成",
            Self::Failed => "失败",
            Self::Canceled => "已取消",
        }
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
    /// 上一次收到进度更新的时刻，用来算瞬时速率。
    pub last_update: Option<SystemTime>,
    /// 上一次算出的速率，字节/秒。第一个事件之前没有速率。
    pub speed_bps: Option<f64>,
}

impl TransferItem {
    /// 0.0 ~ 1.0。总数未知时不猜，给 0（进度条靠状态色表示"在跑"）。
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            return if self.status == TransferStatus::Completed {
                1.0
            } else {
                0.0
            };
        }
        (self.transferred as f64 / self.total as f64).clamp(0.0, 1.0) as f32
    }

    /// 百分比，给 [`gpui_component::progress::Progress`]（它要 0~100）。
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
    /// 速率按"距上次更新多久搬了多少"算，只留最后一段，因此拖动进度条与网络抖动
    /// 的反应是即时的；完成与失败不再算速率，留下的那个数字就是最后一段的。
    pub fn record_progress(&mut self, transferred: u64, at: SystemTime) {
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
            self.speed_bps = Some(delta_bytes / delta_secs);
        }
        if self.status == TransferStatus::Queued && transferred > 0 {
            self.status = TransferStatus::Transferring;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(total: u64) -> TransferItem {
        TransferItem {
            id: 1,
            filename: "app.tar.gz".into(),
            direction: TransferDirection::Upload,
            total,
            transferred: 0,
            status: TransferStatus::Queued,
            error: None,
            last_update: None,
            speed_bps: None,
        }
    }

    #[test]
    fn progress_is_a_percentage() {
        let mut half = item(1000);
        half.transferred = 250;
        assert_eq!(half.progress(), 25.0);

        // 对端报了个比总数还大的数（换文件、重传都可能），不能画爆进度条。
        half.transferred = 4000;
        assert_eq!(half.progress(), 100.0);
    }

    #[test]
    fn unknown_total_does_not_invent_a_percentage() {
        let mut unknown = item(0);
        assert_eq!(unknown.progress(), 0.0);
        unknown.transferred = 4096;
        assert_eq!(unknown.progress(), 0.0, "没有总数就不假装知道比例");
        unknown.status = TransferStatus::Completed;
        assert_eq!(unknown.progress(), 1.0 * 100.0, "完成就是完成");
    }

    #[test]
    fn speed_comes_from_the_delta() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let mut item = item(10 * 1024 * 1024);

        item.record_progress(1024 * 1024, start);
        assert_eq!(item.speed_bps, None, "第一个事件没有可比的前一次");
        assert_eq!(item.status, TransferStatus::Transferring, "动了就算在传");

        item.record_progress(
            1024 * 1024 + 2 * 1024 * 1024,
            start + Duration::from_secs(1),
        );
        assert_eq!(item.speed_bps, Some(2.0 * 1024.0 * 1024.0));
        assert_eq!(item.speed_label(), "2 MB/s");
    }

    #[test]
    fn progress_never_goes_backwards() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let mut item = item(100);
        item.record_progress(80, start);
        item.record_progress(20, start + Duration::from_secs(1));
        assert_eq!(item.transferred, 80);
    }

    #[test]
    fn only_open_states_count_as_active() {
        assert!(TransferStatus::Queued.is_active());
        assert!(TransferStatus::Transferring.is_active());
        assert!(!TransferStatus::Completed.is_active());
        assert!(!TransferStatus::Failed.is_active());
        assert!(!TransferStatus::Canceled.is_active());
        assert_eq!(TransferStatus::Canceled.label(), "已取消");
    }
}
