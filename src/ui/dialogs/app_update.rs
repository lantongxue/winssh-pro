//! 更新可用弹窗：展示发版日志，下载、校验、安装。
//!
//! 主窗口与设置窗口共用同一份流程状态（[`crate::service::updater::state`]），
//! 所以从哪一边打开、关掉弹窗再打开，进度都还在。
//!
//! 发版日志就是 GitHub Release 的正文 —— 由 CI 里的 git-cliff 按提交类型
//! 生成（见仓库根的 `cliff.toml`），云函数的更新接口原样转过来。这里用
//! gpui-kit 的 Markdown 渲染，不自己解析。

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IconName,
    StyledExt as _, WindowExt as _,
    notification::Notification,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    progress::Progress,
    text::TextView,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};
use rust_i18n::t;

use crate::service::updater::{
    self, AvailableUpdate, InstallResult, ManualReason, Phase, State, format_bytes,
    state as update_state,
};
use crate::service::update as update_prefs;
use crate::ui::theme::palette;

/// 更新弹窗的正文实体。
pub struct UpdateBody {
    update: AvailableUpdate,
    state: std::sync::Arc<State>,
}

impl UpdateBody {
    fn new(update: AvailableUpdate) -> Self {
        Self {
            update,
            state: update_state::state(),
        }
    }
}

impl Render for UpdateBody {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let update = self.update.clone();
        let phase = self.state.phase.clone();

        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Info).text_color(palette::blue()))
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(t!("update.available", version => update.version.clone())),
                    ),
            )
            .children(match &phase {
                Phase::Downloading | Phase::Verifying => {
                    let fraction = self.state.fraction();
                    let label = if phase == Phase::Verifying {
                        t!("update.verifying").to_string()
                    } else {
                        t!(
                            "update.downloading",
                            size => format_bytes(self.state.downloaded),
                            speed => format!("{}/s", format_bytes(self.state.speed))
                        )
                        .to_string()
                    };
                    vec![
                        Progress::new("app-update-progress")
                            .value(fraction.unwrap_or(0.0) * 100.0)
                            .into_any_element(),
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(label))
                            .into_any_element(),
                    ]
                }
                Phase::Ready => vec![div()
                    .text_xs()
                    .text_color(palette::green())
                    .child(t!("update.ready"))
                    .into_any_element()],
                Phase::Installing => vec![div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(t!("update.installing"))
                    .into_any_element()],
                Phase::Failed(error) => vec![div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(SharedString::from(error.clone()))
                    .into_any_element()],
                Phase::Manual(reason) => vec![div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(manual_message(*reason)))
                    .into_any_element()],
                Phase::Idle => vec![],
            })
            .child(
                // 日志可能很长：固定一个高度，交给 TextView 自己的 scrollable
                // 模式滚动（它要求父容器高度确定，里面有滚动条与虚拟化）。
                div()
                    .h(px(260.))
                    .w_full()
                    .overflow_hidden()
                    .child(
                        TextView::markdown("app-update-notes", update.notes.clone())
                            .scrollable(true),
                    ),
            )
            .children(if update.asset.is_none() {
                // 没有适用于本平台的产物：只能打开发布页手动下载。
                Some(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(t!("update.no_asset"))
                        .into_any_element(),
                )
            } else {
                None
            })
    }
}

/// 更新可用弹窗。
pub struct UpdateDialog;

impl UpdateDialog {
    /// 弹出"发现新版本"弹窗。
    pub fn open(update: AvailableUpdate, window: &mut Window, cx: &mut App) {
        let body = cx.new(|_| UpdateBody::new(update.clone()));

        // 订阅下载状态，驱动正文重绘（与 mihomo 下载弹窗同一套）。
        let mut changes = updater::subscribe();
        let body_for_task = body.clone();
        body.update(cx, |_, cx| {
            cx.spawn_in(window, async move |_, cx| {
                while changes.changed().await.is_ok() {
                    let state = (*changes.borrow_and_update()).clone();
                    let gone = body_for_task
                        .update_in(cx, |this, _, cx| {
                            this.state = state;
                            cx.notify();
                        })
                        .is_err();
                    if gone {
                        break;
                    }
                }
            })
            .detach();
        });

        let blue = palette::blue();
        window.open_dialog(cx, move |dialog, _window, app| {
            let accent = ButtonCustomVariant::new(app)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));
            let phase = update_state::state().phase.clone();
            let has_asset = update.asset.is_some();
            let release_url = update.release_url.clone();
            let version = update.version.clone();

            let body_download = body.clone();
            let body_install = body.clone();
            let body_version = version.clone();
            let release_for_manual = release_url.clone();

            dialog
                .w(px(560.))
                .title(t!("update.title"))
                .child(body.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(app.theme().border)
                        .child(
                            Button::new("app-update-later")
                                .ghost()
                                .label(t!("update.later"))
                                .on_click(move |_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("app-update-skip")
                                .ghost()
                                .label(t!("update.skip"))
                                .on_click(move |_, window, cx| {
                                    persist_skip(body_version.clone(), cx);
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(if has_asset {
                            let (label, enabled) = match phase {
                                Phase::Idle | Phase::Failed(_) => (t!("update.download"), true),
                                Phase::Ready => (t!("update.install"), true),
                                Phase::Manual(_) => (t!("update.open_release"), true),
                                Phase::Downloading | Phase::Verifying | Phase::Installing => {
                                    (t!("update.working"), false)
                                }
                            };
                            Button::new("app-update-primary")
                                .custom(accent)
                                .bg(blue)
                                .rounded(app.theme().radius)
                                .label(label)
                                .disabled(!enabled)
                                .on_click(move |_, window, cx| {
                                    if matches!(update_state::state().phase, Phase::Manual(_)) {
                                        // 不能自动装：把用户送去发布页。
                                        cx.open_url(&release_for_manual);
                                    } else if matches!(update_state::state().phase, Phase::Ready) {
                                        install_ready(body_install.clone(), window, cx);
                                    } else {
                                        body_download.update(cx, |this, cx| {
                                            updater::begin(this.update.clone());
                                            cx.notify();
                                        });
                                    }
                                })
                        } else {
                            Button::new("app-update-release-page")
                                .custom(accent)
                                .bg(blue)
                                .rounded(app.theme().radius)
                                .label(t!("update.open_release"))
                                .on_click(move |_, _window, cx| {
                                    cx.open_url(&release_url);
                                })
                        }),
                )
        });
    }
}

/// 从"已就绪"状态发起安装。安装里的提权/子进程是阻塞操作，放后台线程。
fn install_ready(body: Entity<UpdateBody>, window: &mut Window, cx: &mut App) {
    let state = update_state::state();
    let Some(package) = state.package.clone() else {
        return;
    };
    let Some(kind) = body.read(cx).update.asset.as_ref().map(|asset| asset.kind.clone()) else {
        return;
    };
    update_state::mark_installing();
    // 安装里的提权/子进程是阻塞操作，放后台线程跑；`background_spawn` 的
    // future 必须 Send，装不进 `AsyncApp` 那一手回窗，所以用 `spawn_in` ——
    // 它拿到的句柄回到界面线程后再写状态。
    let result = cx.background_spawn(async move {
        crate::service::UpdateService::install_update(&kind, &package)
    });
    window.close_dialog(cx);
    window
        .spawn(cx, async move |cx| {
            let result = result.await;
            _ = cx.update(|window, cx| finish_install(result, window, cx));
        })
        .detach();
}

/// 安装结果怎么收尾。
///
/// - Windows：安装包已经在跑，旧进程必须退出，安装包自己在装完后按
///   `[Run]` 那条重新拉起应用；
/// - macOS：新 bundle 已经替换并 `open`，旧进程退出即可；
/// - Linux：文件已经换好，提示用户重启。
fn finish_install(result: anyhow::Result<InstallResult>, window: &mut Window, cx: &mut App) {
    match result {
        Ok(InstallResult::RestartRequired) => cx.quit(),
        Ok(InstallResult::InstalledRestartManually) => {
            window.push_notification(t!("update.installed_restart"), cx);
            window.close_dialog(cx);
        }
        Ok(InstallResult::Manual(reason)) => {
            let message = manual_message(reason).to_string();
            update_state::mark_failed(message.clone());
            window.push_notification(Notification::warning(message), cx);
        }
        Err(error) => {
            let message = format!("{error:#}");
            update_state::mark_failed(message.clone());
            window.push_notification(Notification::error(message), cx);
        }
    }
}

/// 不能自动安装的原因翻成用户看得懂的句子。
fn manual_message(reason: ManualReason) -> String {
    match reason {
        ManualReason::PortableBuild => t!("update.manual.portable").to_string(),
        ManualReason::NotPackageManaged => t!("update.manual.not_package_managed").to_string(),
        ManualReason::NotAppBundle => t!("update.manual.not_app").to_string(),
        ManualReason::BundleNotWritable => t!("update.manual.not_writable").to_string(),
        ManualReason::AuthCancelled => t!("update.manual.auth_cancelled").to_string(),
        ManualReason::NotSigned => t!("update.manual.not_signed").to_string(),
    }
}

/// 记住"跳过这个版本"。设置窗口与主窗口都调它。
pub fn persist_skip(version: String, cx: &mut App) {
    let mut preferences = update_prefs::global();
    preferences.skipped_version = version;
    update_prefs::set_global(preferences.clone());
    cx.background_spawn(async move {
        if let Err(error) = crate::service::SettingService::open()
            .save_update_preferences(preferences)
            .await
        {
            log::error!("更新偏好存不下去：{error:#}");
        }
    })
    .detach();
}
