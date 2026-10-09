//! mihomo 内核下载确认与进度弹窗。
//!
//! 在检测到本地缺少 mihomo 内核文件时弹出，向用户展示内核版本、
//! 目标系统架构、下载资源文件名及保存路径，待用户点击"立即下载"
//! 后启动下载，并在弹窗内实时展示下载进度、下载速率、解压与就绪状态。
//! 下载成功后自动关闭弹窗并触发代理启动回调。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IconName, Sizable as _,
    StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    progress::Progress,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};
use rust_i18n::t;

use crate::clash::downloader::{
    self, DownloadPhase, DownloadState, MIHOMO_TAG, Plan, format_bytes,
};
use crate::ui::theme::{AppIcon, palette};

/// 成功下载并就绪后的回调处理函数。
pub type MihomoReadyHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 下载确认与进度弹窗的正文实体。
struct MihomoDownloadBody {
    plan: Option<Plan>,
    plan_error: Option<SharedString>,
    state: Arc<DownloadState>,
    on_ready: Option<MihomoReadyHandler>,
}

impl MihomoDownloadBody {
    fn new(on_ready: Option<MihomoReadyHandler>) -> Self {
        let (plan, plan_error) = match downloader::plan() {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(SharedString::from(format!("{e:#}")))),
        };
        Self {
            plan,
            plan_error,
            state: downloader::state(),
            on_ready,
        }
    }

    /// 开始下载。
    fn start_download(&mut self, cx: &mut Context<Self>) {
        downloader::begin();
        self.state = downloader::state();
        cx.notify();
    }

    /// 取消下载并关闭弹窗。
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        downloader::cancel();
        window.close_dialog(cx);
    }

    /// 仅关闭弹窗，不触发启动。
    fn close_only(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
    }

    /// 重试下载。
    fn retry(&mut self, cx: &mut Context<Self>) {
        downloader::reset();
        downloader::begin();
        self.state = downloader::state();
        cx.notify();
    }

    /// 触发就绪回调并关闭弹窗（用户手动点击启动代理）。
    fn complete_and_launch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handler = self.on_ready.take();
        window.close_dialog(cx);
        if let Some(handler) = handler {
            handler(window, cx);
        }
    }

    /// 接收下载器状态更新。
    fn on_download_state(&mut self, state: Arc<DownloadState>, cx: &mut Context<Self>) {
        self.state = state;
        cx.notify();
    }
}

impl Render for MihomoDownloadBody {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let phase = &self.state.phase;
        v_flex().gap_3().p_4().child(match phase {
            DownloadPhase::Idle => self.render_confirm(cx).into_any_element(),
            DownloadPhase::Downloading | DownloadPhase::Unpacking => {
                self.render_progress(cx).into_any_element()
            }
            DownloadPhase::Done => self.render_done(cx).into_any_element(),
            DownloadPhase::Failed(err) => self.render_failed(err.clone(), cx).into_any_element(),
        })
    }
}

impl MihomoDownloadBody {
    /// 阶段 1：确认下载信息展示
    fn render_confirm(&self, cx: &Context<Self>) -> impl IntoElement {
        let mono_font = cx.theme().mono_font_family.clone();
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!("clash.download.confirm_desc"))),
            )
            .child(
                div()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .bg(palette::recessed(cx))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        v_flex()
                            .gap_2()
                            .child(info_row(
                                t!("clash.download.version").to_string(),
                                MIHOMO_TAG.to_string(),
                                mono_font.clone(),
                                cx,
                            ))
                            .child(info_row(
                                t!("clash.download.platform").to_string(),
                                format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                                mono_font.clone(),
                                cx,
                            ))
                            .children(self.plan.as_ref().map(|p| {
                                info_row(
                                    t!("clash.download.asset").to_string(),
                                    p.asset.clone(),
                                    mono_font.clone(),
                                    cx,
                                )
                            }))
                            .children(self.plan.as_ref().map(|p| {
                                info_row(
                                    t!("clash.download.target_dir").to_string(),
                                    p.target.display().to_string(),
                                    mono_font.clone(),
                                    cx,
                                )
                            })),
                    ),
            )
            .children(self.plan_error.as_ref().map(|err| {
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(err.clone())
            }))
    }

    /// 阶段 2：实时下载进度展示
    fn render_progress(&self, cx: &Context<Self>) -> impl IntoElement {
        let is_unpacking = self.state.phase == DownloadPhase::Unpacking;
        let downloaded = self.state.downloaded;
        let total = self.state.total;
        let speed = self.state.speed;
        let fraction = self.state.fraction();
        let teal = palette::teal();

        let progress_value = if is_unpacking {
            100.0
        } else {
            fraction.unwrap_or(0.0) * 100.0
        };

        let status_desc = if is_unpacking {
            SharedString::from(t!("clash.download.unpacking"))
        } else if downloaded == 0 && total.is_none() {
            SharedString::from(t!("clash.download.connecting"))
        } else {
            SharedString::from(t!("clash.download.downloading_subtitle"))
        };

        v_flex()
            .gap_3p5()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(status_desc),
            )
            // 进度条容器
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        Progress::new("mihomo-download-progress-bar")
                            .w_full()
                            .color(teal)
                            .value(progress_value)
                            .loading(!is_unpacking && fraction.is_none()),
                    )
                    // 状态与速率行
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(
                                div()
                                    .text_color(cx.theme().foreground)
                                    .child(if is_unpacking {
                                        SharedString::from(t!("clash.download.unpacking"))
                                    } else {
                                        match total {
                                            Some(total_bytes) => format!(
                                                "{} / {}",
                                                format_bytes(downloaded),
                                                format_bytes(total_bytes)
                                            )
                                            .into(),
                                            None => format_bytes(downloaded).into(),
                                        }
                                    }),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .children(if !is_unpacking && speed > 0 {
                                        Some(
                                            div()
                                                .text_color(teal)
                                                .child(format!("{}/s", format_bytes(speed))),
                                        )
                                    } else {
                                        None
                                    })
                                    .children(if !is_unpacking {
                                        fraction.map(|f| {
                                            div()
                                                .font_bold()
                                                .text_color(cx.theme().foreground)
                                                .child(format!("{:.0}%", f * 100.0))
                                        })
                                    } else {
                                        None
                                    }),
                            ),
                    ),
            )
            // 文件提示条
            .children(self.plan.as_ref().map(|p| {
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .p_2()
                    .rounded(cx.theme().radius)
                    .bg(palette::recessed(cx))
                    .child(
                        Icon::new(AppIcon::Archive)
                            .with_size(px(13.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(p.asset.clone())),
                    )
            }))
    }

    /// 阶段 3：下载就绪
    fn render_done(&self, cx: &Context<Self>) -> impl IntoElement {
        let green = palette::green();
        v_flex()
            .gap_3()
            .items_center()
            .py_2()
            .child(
                div()
                    .size(px(40.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(green.opacity(0.12))
                    .text_color(green)
                    .child(Icon::new(IconName::Check).with_size(px(22.))),
            )
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(SharedString::from(t!("clash.download.done_title"))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!("clash.download.done_desc"))),
            )
    }

    /// 阶段 4：下载失败
    fn render_failed(&self, err: SharedString, cx: &Context<Self>) -> impl IntoElement {
        let red = cx.theme().danger;
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2p5()
                    .items_center()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .bg(red.opacity(0.08))
                    .border_1()
                    .border_color(red.opacity(0.25))
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .with_size(px(18.))
                            .text_color(red),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(err),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("请检查网络连接或代理设置后重试。"),
            )
    }
}

/// 一行"标签 + 等宽值"说明。
fn info_row(label: String, value: String, mono_font: SharedString, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .w(px(72.))
                .flex_shrink_0()
                .child(SharedString::from(label)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(mono_font)
                .text_xs()
                .text_color(cx.theme().foreground)
                .child(SharedString::from(value)),
        )
}

/// 弹窗主类型：mihomo 内核下载确认与进度弹窗。
pub struct MihomoDownloadDialog;

impl MihomoDownloadDialog {
    /// 弹出下载确认与进度弹窗。
    ///
    /// 若内核已经就绪，则直接触发 `on_ready`（如果提供）。
    pub fn open(on_ready: Option<MihomoReadyHandler>, window: &mut Window, cx: &mut App) {
        if downloader::is_installed() {
            if let Some(handler) = on_ready {
                handler(window, cx);
            }
            return;
        }

        let body = cx.new(|_| MihomoDownloadBody::new(on_ready));

        // 监听下载器进度通道并通知视图更新
        let mut changes = downloader::subscribe();
        let body_for_task = body.clone();
        body.update(cx, |_, cx| {
            cx.spawn_in(window, async move |_, cx| {
                while changes.changed().await.is_ok() {
                    let state = (*changes.borrow_and_update()).clone();
                    let gone = body_for_task
                        .update_in(cx, |this, _, cx| {
                            this.on_download_state(state, cx);
                        })
                        .is_err();
                    if gone {
                        break;
                    }
                }
            })
            .detach();
        });

        let teal = palette::teal();
        window.open_dialog(cx, move |dialog, _win, app| {
            let cancel_body = body.clone();
            let start_body = body.clone();
            let retry_body = body.clone();
            let launch_body = body.clone();

            let current_body = body.read(app);
            let phase = current_body.state.phase.clone();
            let is_unpacking = phase == DownloadPhase::Unpacking;

            let download_variant = ButtonCustomVariant::new(app)
                .color(teal)
                .foreground(palette::on_accent())
                .hover(teal.lighten(0.08))
                .active(teal.darken(0.08));

            dialog
                .w(px(460.))
                .title(render_header(&phase, app))
                .child(body.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(app.theme().border)
                        .children(match phase {
                            DownloadPhase::Idle => vec![
                                Button::new("mihomo-download-cancel")
                                    .ghost()
                                    .label(t!("clash.download.btn_cancel"))
                                    .on_click(move |_, window, cx| {
                                        cancel_body.update(cx, |this, cx| {
                                            this.cancel(window, cx);
                                        });
                                    })
                                    .into_any_element(),
                                Button::new("mihomo-download-start")
                                    .custom(download_variant)
                                    .bg(teal)
                                    .rounded(app.theme().radius)
                                    .label(t!("clash.download.btn_download"))
                                    .on_click(move |_, _, cx| {
                                        start_body.update(cx, |this, cx| {
                                            this.start_download(cx);
                                        });
                                    })
                                    .into_any_element(),
                            ],
                            DownloadPhase::Downloading | DownloadPhase::Unpacking => vec![
                                Button::new("mihomo-download-stop")
                                    .ghost()
                                    .disabled(is_unpacking)
                                    .label(t!("clash.download.btn_cancel"))
                                    .on_click(move |_, window, cx| {
                                        if !is_unpacking {
                                            cancel_body.update(cx, |this, cx| {
                                                this.cancel(window, cx);
                                            });
                                        }
                                    })
                                    .into_any_element(),
                            ],
                            DownloadPhase::Done => vec![
                                Button::new("mihomo-download-done-close")
                                    .ghost()
                                    .label(t!("clash.download.btn_close"))
                                    .on_click(move |_, window, cx| {
                                        cancel_body.update(cx, |this, cx| {
                                            this.close_only(window, cx);
                                        });
                                    })
                                    .into_any_element(),
                                Button::new("mihomo-download-launch")
                                    .custom(download_variant)
                                    .bg(teal)
                                    .rounded(app.theme().radius)
                                    .label(t!("clash.download.btn_start"))
                                    .on_click(move |_, window, cx| {
                                        launch_body.update(cx, |this, cx| {
                                            this.complete_and_launch(window, cx);
                                        });
                                    })
                                    .into_any_element(),
                            ],
                            DownloadPhase::Failed(_) => vec![
                                Button::new("mihomo-download-fail-close")
                                    .ghost()
                                    .label(t!("clash.download.btn_cancel"))
                                    .on_click(move |_, window, cx| {
                                        cancel_body.update(cx, |this, cx| {
                                            this.cancel(window, cx);
                                        });
                                    })
                                    .into_any_element(),
                                Button::new("mihomo-download-retry")
                                    .custom(download_variant)
                                    .bg(teal)
                                    .rounded(app.theme().radius)
                                    .label(t!("clash.download.btn_retry"))
                                    .on_click(move |_, _, cx| {
                                        retry_body.update(cx, |this, cx| {
                                            this.retry(cx);
                                        });
                                    })
                                    .into_any_element(),
                            ],
                        }),
                )
        });
    }
}

/// 顶部标题行：图标与主副标题。
fn render_header(phase: &DownloadPhase, cx: &App) -> impl IntoElement {
    let teal = palette::teal();
    let (icon, title, subtitle) = match phase {
        DownloadPhase::Idle => (
            Icon::new(AppIcon::Download)
                .with_size(px(16.))
                .into_any_element(),
            t!("clash.download.title").to_string(),
            t!("clash.download.subtitle").to_string(),
        ),
        DownloadPhase::Downloading | DownloadPhase::Unpacking => (
            Icon::new(AppIcon::Download)
                .with_size(px(16.))
                .into_any_element(),
            t!("clash.download.downloading_title").to_string(),
            t!("clash.download.downloading_subtitle").to_string(),
        ),
        DownloadPhase::Done => (
            Icon::new(IconName::Check)
                .with_size(px(16.))
                .into_any_element(),
            t!("clash.download.done_title").to_string(),
            t!("clash.download.done_desc").to_string(),
        ),
        DownloadPhase::Failed(_) => (
            Icon::new(IconName::TriangleAlert)
                .with_size(px(16.))
                .into_any_element(),
            t!("clash.download.failed_title").to_string(),
            "下载或解压过程中发生错误".to_string(),
        ),
    };

    h_flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(cx.theme().radius)
                .bg(teal.opacity(0.1))
                .border_1()
                .border_color(teal.opacity(0.3))
                .text_color(teal)
                .child(icon),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(title)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(subtitle)),
                ),
        )
}
