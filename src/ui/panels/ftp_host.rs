//! 独立 FTP 会话面板：一台 `HostKind::Ftp` 主机开出来的那个标签页。
//!
//! 形状和 [`super::DockedSftpPanel`] 同构 —— 都只是一个把
//! [`SftpPanel`] 铺成整页的壳 —— 但拨号那一侧正好相反：
//!
//! - **认证用账号密码**。FTP 协议没有主机公钥这一说，也没有"密钥 / Agent"
//!   那两档认证 —— 表单里 FTP 主机的认证下拉只给"密码"，这里直接把那一对
//!   值交给 `opendal::services::Ftp`。所以也没有"主机信任确认框"这一段
//!   流程，连不上就是连不上，原因直接落进空状态。
//! - **走 OpenDAL，不走 russh**。因为项目信任库、传输池、被动模式由
//!   OpenDAL 收口，这里不再引第二个 FTP 客户端库。
//!
//! 与 [`super::DockedSftpPanel`] 的关系是"同一种壳，两种芯"：
//! 渲染 / 文件面板本都一样，只是构造里把 [`FtpService`] 作为
//! [`crate::service::FileService::Ftp`] 喂给 [`SftpPanel`]，面板里
//! 那枚徽标就会标成 "FTP" 而不是 "SFTP"。

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, TabGroup, panel_handle},
    h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    WeakEntity, Window, div, px,
};
use rust_i18n::t;

use super::{SessionEvent, SftpPanel, SftpPanelEvent};
use crate::{
    host::Host,
    service::{FileService, FtpService},
    ui::theme::palette,
};

/// 标签页上那个颜色点的直径，和别的会话同一档。
const TAB_DOT_SIZE: gpui_kit::Pixels = px(6.);

/// 独立 FTP 会话。握着那条 FTP 连接，开着它的文件面板铺满整页。
pub struct DockedFtpPanel {
    host: Host,
    /// 面板本体。连接本身由 [`FtpService`] 自己管（OpenDAL 内部连接池），
    /// 这一侧只持有"现在连没连上"的那份凭据。
    service_connected: bool,
    /// 正在拨号中。"再连一次"那颗按钮靠它知道要灰着。
    connecting: bool,
    /// FTP 文件栏。永远开着，撑满整个标签页。
    sftp_panel: Entity<SftpPanel>,
    /// 所在 Dock。编辑器面板要从 SFTP 栏里开进中心区时经它。
    dock_area: WeakEntity<DockArea>,
    focus_handle: FocusHandle,
    /// 上一次报出去的存活状态。
    alive: bool,
}

impl EventEmitter<PanelEvent> for DockedFtpPanel {}
impl EventEmitter<SessionEvent> for DockedFtpPanel {}

impl Focusable for DockedFtpPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl DockedFtpPanel {
    /// 开一条独立 FTP 会话：面板一建好就把拨号调起来。
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
                service_connected: false,
                connecting: false,
                sftp_panel: sftp_panel.clone(),
                dock_area,
                focus_handle: cx.focus_handle(),
                alive: false,
            };
            sftp_panel.update(cx, |panel, cx| panel.set_open(true, window, cx));
            Self::subscribe_sftp(&sftp_panel, window, cx);
            this.connect(window, cx);
            this
        })
    }

    /// 这条会话还活着吗：FTP 通道开起来了就算连着。
    pub fn alive(&self, _cx: &App) -> bool {
        self.alive
    }

    // ─── 拨号 ──────────────────────────────────────────────────────────────

    /// 拨 FTP：账号密码直接交给 Opendal 的 builder，绕开 SSH 那一套。
    ///
    /// 失败就只摆原因 + 一颗"再连一次" —— 没有主机信任确认框（FTP 没有
    /// 主机指纹可验），没有回落本机（见 [`super::DockedSftpPanel`] 上"不回落"
    /// 的同一个理由）。
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        self.connecting = true;
        cx.notify();
        let host = self.host.clone();
        let panel = cx.entity().downgrade();
        cx.spawn_in(window, async move |_, cx| {
            // 构造该服务做的事只是参数装配：build 在 `FtpService::connect` 里同步
            // 完成，真正的网络动作（连接/登录/列表）都推迟到面板第一次调用 ——
            // 这一步的"连上"只是参数可信，不是通道就绪。
            let outcome = FtpService::connect(&host);
            _ = panel.update_in(cx, |this, window, cx| match outcome {
                Ok(service) => {
                    this.connecting = false;
                    this.service_connected = true;
                    this.sftp_panel.update(cx, |panel, cx| {
                        panel.attach(Some(FileService::Ftp(service)), None, window, cx);
                    });
                    this.set_alive(true, cx);
                    cx.notify();
                }
                Err(error) => this.finish_error(format!("{error:#}"), window, cx),
            });
        })
        .detach();
    }

    /// 拨号 / 服务构造失败的收尾：原因摆进面板空状态，颜色点记灰。
    fn finish_error(&mut self, error: String, window: &mut Window, cx: &mut Context<Self>) {
        self.connecting = false;
        self.service_connected = false;
        self.sftp_panel.update(cx, |panel, cx| {
            panel.attach(None, Some(SharedString::from(error.clone())), window, cx)
        });
        self.set_alive(false, cx);
        cx.notify();
    }

    /// 存活状态翻面了就报一声，没变就不报 —— 和终端、独立 SFTP 同一条口径。
    fn set_alive(&mut self, alive: bool, cx: &mut Context<Self>) {
        if self.alive == alive {
            return;
        }
        self.alive = alive;
        cx.emit(SessionEvent::LivenessChanged);
    }

    // ─── 子面板 ────────────────────────────────────────────────────────────

    /// 面板向外报的事。与 [`super::DockedSftpPanel`] 同一组事件被翻译成
    /// 本面板自己的动作 —— `SendCommand` / `SetHeight` 同样没有终端可发，
    /// `Reconnect` 也几乎不可能失败（构造轻活），但仍走"重新 connect"。
    fn subscribe_sftp(sftp_panel: &Entity<SftpPanel>, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            sftp_panel,
            window,
            |this: &mut Self, _, event: &SftpPanelEvent, window, cx| match event {
                SftpPanelEvent::Close => this.close(window, cx),
                SftpPanelEvent::Reconnect => {
                    this.service_connected = false;
                    this.connecting = false;
                    this.connect(window, cx);
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
                SftpPanelEvent::SendCommand(_) | SftpPanelEvent::SetHeight(_) => {}
            },
        )
        .detach();
    }

    /// 关掉自己这个标签页。同 SFTP：走 `DockArea::remove_panel`，帧末执行。
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

impl BasePanel for DockedFtpPanel {
    fn panel_name(&self) -> &'static str {
        "DockedFtpPanel"
    }

    fn on_added_to(
        &mut self,
        _: WeakEntity<TabGroup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.focus_self(window);
        cx.notify();
    }

    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.alive = false;
        cx.emit(SessionEvent::Ended);
    }
}

impl Panel for DockedFtpPanel {
    /// 标签页和别的会话一致：一颗颜色点、会话名、一个 "FTP" 的小尾巴、关闭。
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
                Button::new("ftp-host-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("ftp_host.close_session"))
                    .accessibility_id(t!("ftp_host.close_session"))
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

impl Render for DockedFtpPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("ftp-host-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.sftp_panel.clone()),
            )
    }
}
