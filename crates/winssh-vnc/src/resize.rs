//! 跟随画布调整远端分辨率：合并尺寸变化，一次只保留一个线上请求。
//! `next` 被 select 取消时，请求 future 仍留在这里，不能反复撤销已发出的请求。

use std::time::Duration;

use futures::future::BoxFuture;
use tokio::time::Instant;
use vnc::{DesktopLayout, ResizeError, VncClient};

const SETTLE_TIME: Duration = Duration::from_millis(180);

pub(super) fn supports_resize(layout: &DesktopLayout) -> bool {
    matches!(layout.screens.as_slice(), [screen]
        if screen.x == 0 && screen.y == 0
            && screen.width == layout.width && screen.height == layout.height)
}

#[derive(Default)]
pub(super) struct DesktopResize {
    target: Option<(u16, u16)>,
    not_before: Option<Instant>,
    attempted: Option<(u16, u16)>,
    pending: Option<BoxFuture<'static, Result<DesktopLayout, ResizeError>>>,
    /// 超时或拒绝后不自动反复尝试，避免与服务器或其他查看者争抢桌面。
    blocked: bool,
}

impl DesktopResize {
    pub(super) fn set_target(&mut self, target: Option<(u16, u16)>) {
        if self.target != target {
            self.target = target;
            self.not_before = target.map(|_| Instant::now() + SETTLE_TIME);
            self.attempted = None;
        }
        // 关闭跟随只丢掉尚未发出的尺寸；已在线上的请求继续接收确认。
    }

    pub(super) async fn next(&mut self, client: &VncClient) -> Result<(), ResizeError> {
        loop {
            if let Some(pending) = &mut self.pending {
                let result = pending.await;
                self.pending = None;
                // 尺寸过大时，窗口缩小后可以再试；其他失败要重连后重新协商。
                if let Err(error) = &result {
                    self.blocked = !matches!(error, ResizeError::InvalidDimensions);
                }
                return result.map(|_| ());
            }
            let Some(target) = self
                .target
                .filter(|target| !self.blocked && self.attempted != Some(*target))
            else {
                return std::future::pending().await;
            };
            if let Some(deadline) = self.not_before {
                tokio::time::sleep_until(deadline).await;
            }
            let Some(layout) = client.desktop_layout().filter(supports_resize) else {
                // 不猜能力、不发探测命令。收到桌面布局事件后，select 会再次轮询这里。
                return std::future::pending().await;
            };
            self.attempted = Some(target);
            if (layout.width, layout.height) == target {
                return Ok(());
            }
            let client = client.clone();
            self.pending = Some(Box::pin(async move {
                client.resize_desktop(target.0, target.1).await
            }));
        }
    }
}
