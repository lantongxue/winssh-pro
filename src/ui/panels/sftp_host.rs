//! 独立 SFTP 会话面板：一台 `HostKind::Sftp` 主机开出来的那个标签页。
//!
//! 和终端标签页的关系是"共用一条拨号，不开 shell"：
//!
//! | 层 | 归谁 |
//! | --- | --- |
//! | 标签页（名字、颜色点、关闭） | [`Panel::title`] |
//! | SFTP 双栏文件面板 | [`SftpPanel`]，铺满整页 |
//!
//! 往下的拨号与文件操作**全部复用** SSH 那一套：信任检查走同一张
//! `trusted_hosts` 表与同一只确认框；开不开的 SFTP 子系统通道由同一个
//! [`crate::service::SftpService`] 端上来；列目录、上传下载、重命名、chmod、
//! 删除、就地编辑文件，都与 SSH 会话底部那栏一模一样 —— 它就是同一个
//! [`SftpPanel`]，只是这里铺满整页、始终展开。
//!
//! 与终端那一栏的两处真正不同：
//!
//! - **失败不回落本机 shell**。SSH 终端拨不上会顶开一个本机 shell 让整个标签
//!   页仍然能做事；独立 SFTP 不能这么干 —— 那种回落给用户看的是一个**本地**
//!   文件管理器，替他保存下来的 `SFTP` 主机变成"打开这台会看到我自己的盘"，
//!   不如老老实实报失败。所以这里的空状态只挂一句原因和一颗"重新连接"。
//! - **拨号不开 PTY**。`connect_session_trusted` 只建已认证会话、不请求 shell，
//!   再在这条连接上把 SFTP 子系统通道开出来（见 [`Self::connect`]）。那种
//!   服务器上"开一个 shell 就会被拒"的提示根本不成立。
//!
//! 存活状态的判断跟着通道走：SFTP 子系统通道开起来了就算"连着"（主机卡片上
//! 那颗点亮着）；拨不上、认证失败、或者用户自己点掉重连之后还没开起来，都算灰着。
//! 关闭时与终端一样报 [`SessionEvent::Ended`]，由
//! [`crate::ui::views::HostExplorerView`] 把会话点灭掉。

use std::sync::{Arc, Mutex};

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, TabGroup, panel_handle},
    h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    InteractiveElement as _, ParentElement as _, Render, SharedString, Styled as _, WeakEntity,
    Window, div, px,
};
use rust_i18n::t;

use super::{SessionEvent, SftpPanel, SftpPanelEvent};
use crate::{
    host::Host,
    service::{SftpService, SshService, TrustedHostService},
    trusted_host::{HostKeyInfo, TrustPolicy},
    ui::{
        dialogs::{HostTrustDialog, TrustDecision, TrustDecisionHandler},
        theme::palette,
    },
};

/// 标签页上那个颜色点的直径，和终端那一档同样的尺寸。
const TAB_DOT_SIZE: gpui_kit::Pixels = px(6.);

/// 独立 SFTP 会话。握着那条 SSH 连接（没有 shell）、
/// 以及开着它通道的那个 [`SftpPanel`]。
pub struct DockedSftpPanel {
    host: Host,
    /// 已认证的 SSH 会话凭据。拨号只在"还没有"或者"用户明说要重来"时才再跑一次；
    /// 重来时换新的一份，绝不把"曾经连上"当成"现在还连着"。
    ssh: Option<crate::ssh::SshSession>,
    /// 正在拨号中。"再连一次"的那颗按钮靠它知道要灰着。
    connecting: bool,
    /// SFTP 文件栏。永远开着，高和宽撑满整个标签页。
    sftp_panel: Entity<SftpPanel>,
    /// 所在 Dock。编辑器面板要从 SFTP 栏里开进中心区时经它 —— 子面板够不到 Dock，
    /// 这个弱句柄从工作区一路穿下来。
    dock_area: WeakEntity<DockArea>,
    focus_handle: FocusHandle,
    /// 上一次报出去的存活状态，防止"没翻面的变化"一遍遍通知主机列表。
    alive: bool,
}

impl EventEmitter<PanelEvent> for DockedSftpPanel {}
impl EventEmitter<SessionEvent> for DockedSftpPanel {}

impl Focusable for DockedSftpPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl DockedSftpPanel {
    /// 开一条独立 SFTP 会话：面板一建好就把拨号调起来。
    pub fn new(
        host: Host,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let sftp_panel = SftpPanel::new(host.clone(), window, cx);
            let mut this = Self {
                host,
                ssh: None,
                connecting: false,
                sftp_panel: sftp_panel.clone(),
                dock_area,
                focus_handle: cx.focus_handle(),
                alive: false,
            };
            // 面板一建好就始终"展开"：它就是标签页本身，没有"先收起来再点开"。
            sftp_panel.update(cx, |panel, cx| panel.set_open(true, window, cx));
            Self::subscribe_sftp(&sftp_panel, window, cx);
            this.dial_ssh(window, cx);
            this
        })
    }

    /// 这条会话还活着吗：SFTP 通道开起来了就算连着。
    pub fn alive(&self, _cx: &App) -> bool {
        self.alive
    }

    // ─── 拨号 ──────────────────────────────────────────────────────────────

    /// 拨 SSH：带主机密钥信任检查，失败**不**回落本机 —— 那只会是把会话变成
    /// 一个本地文件管理器，结果更难发现（见模块文档）。
    ///
    /// 流程与终端那一段同形：查信任表 → 决定 [`TrustPolicy`] → 先连一次 →
    /// 首次未知主机弹确认框 → 用户确认后再连。其余失败（网络不通、认证失败、
    /// 密钥变更）落成面板空状态里那一行原因。
    fn dial_ssh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        self.connecting = true;
        cx.notify();
        let host = self.host.clone();
        let panel = cx.entity().downgrade();
        cx.spawn_in(window, async move |_, cx| {
            let hostname = host.hostname.to_string();
            let port = host.port;

            let stored_fp = TrustedHostService::open()
                .fingerprint_for(&hostname, port)
                .await
                .ok()
                .flatten();

            let capture = Arc::new(Mutex::new(None::<HostKeyInfo>));
            let trust = match stored_fp {
                Some(fp) => TrustPolicy::Verify { expected: fp },
                None => TrustPolicy::CaptureAndAbort {
                    capture: capture.clone(),
                },
            };

            let outcome = SshService::new().connect_session_trusted(&host, trust).await;

            match outcome {
                Ok(session) => {
                    TrustedHostService::open().touch(&hostname, port).await.ok();
                    _ = panel.update_in(cx, |this, window, cx| {
                        this.ssh = Some(session.clone());
                        this.open_sftp(window, cx);
                    });
                }
                Err(error) => {
                    let info = capture.lock().ok().and_then(|mut s| s.take());
                    _ = panel.update_in(cx, |this, window, cx| match info {
                        Some(info) => this.show_trust_dialog(host.clone(), info, window, cx),
                        None => this.finish_error(format!("{error:#}"), window, cx),
                    });
                }
            }
        })
        .detach();
    }

    /// 首次未知主机的那只确认框。接受就按"带上保存的指纹"或"只信这一次"
    /// 再连一次；拒绝就把那句提示挂进面板空状态。
    fn show_trust_dialog(
        &mut self,
        host: Host,
        info: HostKeyInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.entity().downgrade();
        let accept_info = info.clone();
        let handler: TrustDecisionHandler = std::rc::Rc::new(move |decision, win, app| {
            let panel = panel.clone();
            match decision {
                TrustDecision::Accept { save } => {
                    let info = accept_info.clone();
                    let host = host.clone();
                    let fp = info.fingerprint.clone();
                    win.spawn(app, async move |cx| {
                        if save {
                            TrustedHostService::open().save(&info).await.ok();
                        }
                        let trust = if save {
                            TrustPolicy::Verify { expected: fp }
                        } else {
                            TrustPolicy::TrustOnce
                        };
                        let outcome = SshService::new()
                            .connect_session_trusted(&host, trust)
                            .await;
                        _ = panel.update_in(cx, |this, window, cx| match outcome {
                            Ok(session) => {
                                this.ssh = Some(session.clone());
                                this.open_sftp(window, cx);
                            }
                            Err(error) => this.finish_error(format!("{error:#}"), window, cx),
                        });
                    })
                    .detach();
                }
                TrustDecision::Reject => {
                    _ = panel.update(app, |this, cx| {
                        this.connecting = false;
                        this.sftp_panel.update(cx, |panel, cx| {
                            panel.note_error(SharedString::from(t!("sftp_host.trust_rejected")), cx);
                        });
                        this.set_alive(false, cx);
                        cx.notify();
                    });
                }
            }
        });
        HostTrustDialog::open(&info, handler, window, cx);
    }

    /// 一条已认证的会话到手了（第一次拨号上来、或确认框那头重试成功）：在它上面
    /// 把 SFTP 子系统通道开出来。这一趟的成败与拨号本身无关，所以沿用同一个空状态。
    fn open_sftp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.ssh.clone() else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let outcome = SftpService::connect(session.handle).await;
            _ = this.update_in(cx, |this, window, cx| match outcome {
                Ok(service) => {
                    this.connecting = false;
                    this.sftp_panel.update(cx, |panel, cx| {
                        panel.attach(
                            Some(crate::service::FileService::Sftp(service)),
                            None,
                            window,
                            cx,
                        )
                    });
                    this.set_alive(true, cx);
                    cx.notify();
                }
                Err(error) => this.finish_error(format!("{error:#}"), window, cx),
            });
        })
        .detach();
    }

    /// 任一趟失败（拨不上、认证不过、子系统开不出）的收尾：原因摆进面板空状态，
    /// 颜色点记灰。错误本身一定要是"对端的原话"：sshd 说的那句比我们能写的都准。
    fn finish_error(&mut self, error: String, window: &mut Window, cx: &mut Context<Self>) {
        self.connecting = false;
        self.sftp_panel.update(cx, |panel, cx| {
            panel.attach(None, Some(SharedString::from(error.clone())), window, cx)
        });
        self.set_alive(false, cx);
        cx.notify();
    }

    /// 存活状态翻面了就报一声，没变就不报 —— 和终端同一条口径：
    /// 主机卡片上的点不能只靠这个重画去点亮。
    fn set_alive(&mut self, alive: bool, cx: &mut Context<Self>) {
        if self.alive == alive {
            return;
        }
        self.alive = alive;
        cx.emit(SessionEvent::LivenessChanged);
    }

    // ─── 子面板 ────────────────────────────────────────────────────────────

    /// 底部 SFTP 栏向外报的事。与终端那一栏同一个 [`SftpPanelEvent`]，但这里
    /// **没有终端可写命令**，所以 `SendCommand` 与 `SetHeight` 安静忽略 —— 这边
    /// 没有"跟随终端 cwd"，高度也已经撑满。只有 `Close`、`Reconnect`、`OpenEditor`
    /// 被翻译成本面板自己的动作。
    fn subscribe_sftp(sftp_panel: &Entity<SftpPanel>, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            sftp_panel,
            window,
            |this: &mut Self, _, event: &SftpPanelEvent, window, cx| match event {
                SftpPanelEvent::Close => this.close(window, cx),
                // "重新连接"：把当前凭据都扔掉，重新走一遍 dial（含信任确认）。
                SftpPanelEvent::Reconnect => {
                    this.ssh = None;
                    this.connecting = false;
                    this.dial_ssh(window, cx);
                }
                SftpPanelEvent::OpenEditor(editor) => {
                    if let Some(area) = this.dock_area.upgrade() {
                        let editor = editor.clone();
                        area.update(cx, |area, cx| {
                            area.add_panel_view(
                                panel_handle(editor),
                                DockPlacement::Center,
                                None,
                                window,
                                cx,
                            );
                        });
                    }
                }
                // 没有终端可送命令，也没有"把面板拉得多高"要改 —— 两个都吃掉。
                SftpPanelEvent::SendCommand(_) | SftpPanelEvent::SetHeight(_) => {}
            },
        )
        .detach();
    }

    /// 关掉自己这个标签页。同终端那一档：走 `DockArea::remove_panel`，
    /// 挪到帧末执行，因为调用方的按钮回调正借着这个面板。
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };
        let panel = cx.entity().clone();
        window.defer(cx, move |window, cx| {
            let _ = dock_area.update(cx, |area, cx| area.remove_panel(panel, window, cx));
        });
    }
}

impl BasePanel for DockedSftpPanel {
    fn panel_name(&self) -> &'static str {
        "DockedSftpPanel"
    }

    fn on_added_to(&mut self, _: WeakEntity<TabGroup>, window: &mut Window, cx: &mut Context<Self>) {
        cx.focus_self(window);
        cx.notify();
    }

    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.alive = false;
        cx.emit(SessionEvent::Ended);
    }
}

impl Panel for DockedSftpPanel {
    /// 标签页与终端对齐：一颗颜色点、会话名、一个"SFTP"的小尾巴、关闭。
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_1p5()
            .child(
                div()
                    .flex_shrink_0()
                    .size(TAB_DOT_SIZE)
                    .rounded_full()
                    .bg(match self.alive {
                        true => palette::magenta(),
                        false => palette::idle(cx),
                    }),
            )
            .child(div().min_w_0().truncate().child(self.host.name.clone()))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::teal())
                    .child(self.host.kind.label()),
            )
            .child(
                Button::new("sftp-host-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("sftp_host.close_session"))
                    .accessibility_id(t!("sftp_host.close_session"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.close(window, cx);
                    })),
            )
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for DockedSftpPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("sftp-host-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .child(div().flex_1().min_h_0().min_w_0().child(self.sftp_panel.clone()))
    }
}
