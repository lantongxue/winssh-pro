//! mihomo 内核的"设置"弹窗：系统的 privileged helper、以及它的虚拟网卡
//! （TUN）参数。
//!
//! 从 Clash 侧边栏底部那颗齿轮按钮点出来（[`crate::ui::views::ClashView`]）。
//!
//! 这一层只管两件事，并且分得开：
//!
//! * **helper**：装 / 卸（macOS：launchd 守护）。helper 装好之前拨 TUN 是
//!   不会成功的，所以装这步让用户主动按、按完看结果，而不是埋在 TUN 拨
//!   开关那一下。**卸载会弹一次系统的授权框**跟安装一样（见
//!   [`crate::clash::broker::uninstall`]）。
//! * **TUN 参数**：stack、网卡名、MTU、auto-route / auto-detect-interface。
//!   这一份归用户（应用级偏好，不分订阅），保存走
//!   [`crate::service::SettingService::save_tun_settings`] —— 下一次起
//!   实例 / 下一次 `Reload` 时 `runtime_config` 拿它盖订阅里的同名字段
//!   （理由见 [`crate::clash::tun_settings`]）。
//!
//! ## 校验
//!
//! 只有 MTU 一项是真校验：用户敲的东西必须能解成 `576..=9000` 的整数（IP
//! 层包的最小帧与 jumbo frame 那条线）。网卡名只 strip 空白与分隔符
//! （`/`、`\`、`:`），剩下交给 mihomo —— 它要是连不上这个名字的网卡会在
//! 启动日志里写清楚。

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IndexPath, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    select::{Select, SelectState},
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use rust_i18n::t;

use crate::{
    clash::{TunSettings, TunStack, broker, tun_settings},
    service::SettingService,
    ui::theme::{AppIcon, palette},
};

///  MTU 允许的范围。IP 层最小帧 576、jumbo 帧 9000 是 RFC 791 / 厂商通行
/// 的那两条线，超出去基本就是用户敲错了。
const MTU_MIN: u32 = 576;
const MTU_MAX: u32 = 9000;

/// helper 目前装没装的那一格。`refresh_helper_status` 重读；装/卸的动作
/// 完成时按结果再翻这一格。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelperState {
    /// 装着。
    Installed,
    /// 没装。
    Missing,
    /// 装到一半（一个文件在一个不在）。
    Partial,
}

impl HelperState {
    fn from_status(status: broker::HelperStatus) -> Self {
        if status.installed {
            Self::Installed
        } else if status.plist_present || status.binary_present {
            Self::Partial
        } else {
            Self::Missing
        }
    }
}

/// helper 安装/卸载进行中的标记：用户点了按钮之后到结果出来之前那一格。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelperBusy {
    Idle,
    Installing,
    Uninstalling,
}

/// TUN 设置弹窗的正文实体。
pub struct ClashTunSettingsDialog {
    service: SettingService,
    helper_state: HelperState,
    helper_busy: HelperBusy,
    /// 最近这次装/卸失败的原因。`report` 通过 watch 递回界面。
    helper_error: Option<SharedString>,
    /// 栈那一档的下拉。
    stack: Entity<SelectState<Vec<SharedString>>>,
    /// 网卡名。
    device: Entity<InputState>,
    /// MTU。Input 接字符串，提交时 parse 校验。
    mtu: Entity<InputState>,
    /// auto-route。
    auto_route: bool,
    /// auto-detect-interface。
    auto_detect_interface: bool,
    /// 保存期间挡住重复提交。
    saving: bool,
}

/// 保存完之后给宿主的回调。
pub type ClashTunSavedHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// helper 装没装请求回话走的那条 channel 的载荷。

impl ClashTunSettingsDialog {
    /// 弹出对话框。
    pub fn open(on_saved: ClashTunSavedHandler, window: &mut Window, cx: &mut App) {
        let form = cx.new(|cx| Self::new(window, cx));

        window.open_dialog(cx, move |dialog, window, cx| {
            let blue = palette::blue();
            let submit_variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));

            let submit_form = form.clone();
            let submit_saved = on_saved.clone();

            dialog
                .w(px(560.))
                .max_h(super::max_form_height(window))
                .title(render_header(t!("clash.tun.title").into(), cx))
                .child(form.clone())
                .footer(
                    div()
                        .w_full()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .w_full()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("clash-tun-cancel")
                                        .ghost()
                                        .label(t!("clash.tun.cancel"))
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("clash-tun-submit")
                                        .custom(submit_variant)
                                        .bg(blue)
                                        .rounded(cx.theme().radius)
                                        .label(t!("clash.tun.save"))
                                        .disabled(form.read(cx).saving)
                                        .on_click(move |_, window, cx| {
                                            let outcome = submit_form
                                                .update(cx, |state, cx| state.build_settings(cx));
                                            let settings = match outcome {
                                                Ok(settings) => settings,
                                                Err(message) => {
                                                    window.push_notification(message, cx);
                                                    return;
                                                }
                                            };
                                            submit_form.update(cx, |state, cx| {
                                                state.saving = true;
                                                cx.notify();
                                            });
                                            let service = submit_form.read(cx).service.clone();
                                            let saved_cb = submit_saved.clone();
                                            submit_form.update(cx, |_, cx| {
                                                cx.spawn_in(window, async move |this, cx| {
                                                    let save_result =
                                                        service.save_tun_settings(settings).await;
                                                    let _ =
                                                        this.update_in(cx, |state, window, cx| {
                                                            state.saving = false;
                                                            match save_result {
                                                                Ok(()) => {
                                                                    saved_cb(window, cx);
                                                                    window.close_dialog(cx);
                                                                }
                                                                Err(error) => {
                                                                    window.push_notification(
                                                                        SharedString::from(
                                                                            format!("{error:#}"),
                                                                        ),
                                                                        cx,
                                                                    );
                                                                }
                                                            }
                                                            cx.notify();
                                                        });
                                                })
                                                .detach();
                                            });
                                        }),
                                ),
                        ),
                )
        });
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = tun_settings::current();

        // Select 那一份：从 stack 枚举到标签的映射，存的是索引。
        let stack = cx.new(|cx| {
            SelectState::new(
                stack_labels(),
                Some(IndexPath::new(
                    TunStack::ALL
                        .iter()
                        .position(|known| *known == prefs.stack)
                        .unwrap_or_default(),
                )),
                window,
                cx,
            )
        });

        let device = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("winssh-tun")
                .default_value(prefs.device.clone())
        });

        let mtu = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1500")
                .default_value(prefs.mtu.to_string())
        });

        let helper_state = HelperState::from_status(broker::status());

        Self {
            service: SettingService::open(),
            helper_state,
            helper_busy: HelperBusy::Idle,
            helper_error: None,
            stack,
            device,
            mtu,
            auto_route: prefs.auto_route,
            auto_detect_interface: prefs.auto_detect_interface,
            saving: false,
        }
    }

    /// 把此刻表单里这几样序列化成 [`TunSettings`]。MTU 解不出来弹错。
    fn build_settings(&self, cx: &Context<Self>) -> Result<TunSettings, SharedString> {
        let mtu_text = self.mtu.read(cx).value().trim().to_owned();
        let mtu: u32 = mtu_text.parse().map_err(|_| {
            SharedString::from(t!("clash.tun.mtu_invalid", value => mtu_text.clone()))
        })?;
        if !(MTU_MIN..=MTU_MAX).contains(&mtu) {
            return Err(SharedString::from(t!(
                "clash.tun.mtu_out_of_range",
                min => MTU_MIN,
                max => MTU_MAX,
            )));
        }

        let stack = TunStack::ALL
            .get(
                self.stack
                    .read(cx)
                    .selected_index(cx)
                    .map(|index| index.row)
                    .unwrap_or_default(),
            )
            .copied()
            .unwrap_or_default();

        let device = sanitize_device_name(&self.device.read(cx).value());

        Ok(TunSettings {
            stack,
            device,
            mtu,
            auto_route: self.auto_route,
            auto_detect_interface: self.auto_detect_interface,
        })
    }

    /// 点"安装 helper"那一下。安装的成功与否要落到 helper_state 上 ——
    /// helper 装好之后 `installed` 就翻真；取消/失败的"装着没装"原样。
    ///
    /// 走 GPUI 的 background executor + broker 的**同步**入口：那一头不许
    /// 碰 tokio，GPUI 的 executor 上没有 reactor（见
    /// [`crate::clash::broker::ensure_helper`] 的注释）。
    fn install_helper(&mut self, cx: &mut Context<Self>) {
        if self.helper_busy != HelperBusy::Idle {
            return;
        }
        self.helper_busy = HelperBusy::Installing;
        self.helper_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { broker::ensure_helper() })
                .await;
            this.update(cx, |state, cx| {
                state.helper_busy = HelperBusy::Idle;
                match result {
                    Ok(_) => {
                        state.helper_state = HelperState::Installed;
                        state.helper_error = None;
                    }
                    Err(error) => {
                        state.helper_error = Some(helper_action_error_text(&error, true));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 点"卸载 helper"那一下。
    fn uninstall_helper(&mut self, cx: &mut Context<Self>) {
        if self.helper_busy != HelperBusy::Idle {
            return;
        }
        self.helper_busy = HelperBusy::Uninstalling;
        self.helper_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { broker::remove_helper() })
                .await;
            this.update(cx, |state, cx| {
                state.helper_busy = HelperBusy::Idle;
                match result {
                    Ok(()) => {
                        state.helper_state = HelperState::Missing;
                        state.helper_error = None;
                    }
                    Err(error) => {
                        state.helper_error = Some(helper_action_error_text(&error, false));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 顶部第一段：helper 状态 + 装/卸按钮。
    fn render_helper_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (badge_color, badge_text) = match self.helper_state {
            HelperState::Installed => (palette::green(), t!("clash.tun.helper_installed")),
            HelperState::Missing => (palette::idle(cx), t!("clash.tun.helper_missing")),
            HelperState::Partial => (
                gpui_kit::rgb(0xf59e0b).into(),
                t!("clash.tun.helper_partial"),
            ),
        };
        let busy = self.helper_busy != HelperBusy::Idle;
        let button_label = match (self.helper_state, self.helper_busy) {
            (_, HelperBusy::Installing) => t!("clash.tun.helper_installing"),
            (_, HelperBusy::Uninstalling) => t!("clash.tun.helper_uninstalling"),
            (HelperState::Installed, _) => t!("clash.tun.helper_uninstall"),
            (_, _) => t!("clash.tun.helper_install"),
        };

        v_flex()
            .gap_2()
            .child(field_label(t!("clash.tun.helper_label").into(), cx))
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(div().size(px(7.)).rounded_full().bg(badge_color))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(badge_text)),
                            ),
                    )
                    .child(
                        Button::new("clash-tun-helper-toggle")
                            .ghost()
                            .icon(Icon::new(match self.helper_state {
                                HelperState::Installed if !busy => AppIcon::Trash,
                                _ => AppIcon::Download,
                            }))
                            .label(button_label)
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| match this.helper_state {
                                HelperState::Installed => this.uninstall_helper(cx),
                                _ => this.install_helper(cx),
                            })),
                    ),
            )
            // 装/卸失败的原因原文摆在这儿。装期那条线不必给具体错误 ——
            // 「取消授权」也是一种合理的归宿。
            .when_some(self.helper_error.clone(), |this, message| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(message))
            })
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(helper_caption())),
            )
    }

    /// 中段：TUN 参数表单。
    fn render_tun_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(field_label(t!("clash.tun.section_title").into(), cx))
            .child(field(
                t!("clash.tun.stack_label").into(),
                Select::new(&self.stack).w_full().into_any_element(),
                cx,
            ))
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div().flex_1().child(field(
                            t!("clash.tun.device_label").into(),
                            Input::new(&self.device)
                                .w_full()
                                .cleanable(true)
                                .into_any_element(),
                            cx,
                        )),
                    )
                    .child(
                        div().w(px(140.)).child(field(
                            t!("clash.tun.mtu_label").into(),
                            Input::new(&self.mtu)
                                .w_full()
                                .cleanable(true)
                                .into_any_element(),
                            cx,
                        )),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .child(switch_row(
                        "clash-tun-auto-route",
                        t!("clash.tun.auto_route_label").into(),
                        self.auto_route,
                        cx.listener(|this, on: &bool, _, cx| {
                            this.auto_route = *on;
                            cx.notify();
                        }),
                        cx,
                    ))
                    .child(switch_row(
                        "clash-tun-auto-detect-interface",
                        t!("clash.tun.auto_detect_label").into(),
                        self.auto_detect_interface,
                        cx.listener(|this, on: &bool, _, cx| {
                            this.auto_detect_interface = *on;
                            cx.notify();
                        }),
                        cx,
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!("clash.tun.caption"))),
            )
    }
}

impl Render for ClashTunSettingsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_4()
            .p_4()
            .child(self.render_helper_section(cx))
            .child(div().w_full().h(px(1.)).bg(cx.theme().border))
            .child(self.render_tun_section(cx))
    }
}

// ---------- 小部件 ----------

/// 一颗字段名 + 一颗下挂的输入条。
fn field(label: SharedString, input: gpui_kit::AnyElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1p5()
        .child(field_label(label, cx))
        .child(input)
}

/// 字段名那一行。
fn field_label(label: SharedString, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

/// 标题：图标 + 文档里那段标题。
fn render_header(title: SharedString, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(
            Icon::new(IconName::Settings)
                .size_4()
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            div()
                .text_base()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(title),
        )
}

/// 一个 Switch + 旁边的说明文字。Switch 的小标 id 由调用方给。
fn switch_row(
    id: &'static str,
    label: SharedString,
    checked: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .gap_1p5()
        .items_center()
        .child(Switch::new(id).checked(checked).on_change(on_change))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().foreground)
                .child(label),
        )
}

/// stack 那一档的下拉选项标签。
fn stack_labels() -> Vec<SharedString> {
    TunStack::ALL
        .iter()
        .map(|stack| SharedString::from(stack.as_yaml()))
        .collect()
}

/// 装/卸失败拿给界面看的那句话：取消授权是常见结局，单独一句；别的错
/// 原文带回去（里面常常躺着 launchd / shell 那句关键提示）。
fn helper_action_error_text(error: &anyhow::Error, is_install: bool) -> SharedString {
    if error.downcast_ref::<broker::ElevationCancelled>().is_some() {
        let key = if is_install {
            "clash.tun.helper_install_canceled"
        } else {
            "clash.tun.helper_uninstall_canceled"
        };
        SharedString::from(t!(key))
    } else {
        SharedString::from(format!("{error:#}"))
    }
}

/// helper 状态行下边那句"helper 是干吗的"提示。macOS / Linux 上不带后缀，
/// Windows 上多一句"无需安装"—— 那个平台是命名管道连着一次性 broker 的，
/// 不存在「装好」这个动作。
fn helper_caption() -> String {
    #[cfg(target_os = "macos")]
    {
        t!("clash.tun.helper_caption_macos").to_string()
    }
    #[cfg(target_os = "linux")]
    {
        t!("clash.tun.helper_caption_linux").to_string()
    }
    #[cfg(windows)]
    {
        t!("clash.tun.helper_caption_windows").to_string()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        String::new()
    }
}

/// 网卡名只留下能进 yaml 的字符 —— 空白、控制字符、分隔符都剥掉。
/// 剥完空了就退回出厂档名。
fn sanitize_device_name(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|letter| letter.is_ascii_alphanumeric() || matches!(letter, '-' | '_' | '.'))
        .collect();
    if cleaned.is_empty() {
        TunSettings::default().device
    } else {
        cleaned
    }
}

#[cfg(test)]
#[path = "../../tests/ui/dialogs/clash_tun_settings.rs"]
mod tests;
