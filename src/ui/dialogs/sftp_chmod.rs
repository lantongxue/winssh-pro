//! 改权限对话框：给一个远端条目设 mode（原型里的 "Change Permissions (chmod)"）。
//!
//! 三个常用档位 + 一个自己敲八进制的输入框，和原型一致。这里只负责
//! **把一位数的八进制模式问清楚**：真正发 `setstat` 的是
//! [`crate::service::SftpService::set_mode`]，由面板在回调里调 ——
//! 改完要重列目录，那是面板的事。
//!
//! 校验做两件事：得是 3~4 位八进制数字，而且不能越过 `07777`（setuid / setgid /
//! sticky 那三位也在里面，允许写但要说清）。越界的值发过去只会被对端拒绝，
//! 提前挡住能少一次往返，也少一句听不懂的英文报错。

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, Hsla, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Colorize as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};

use crate::ui::theme::{AppIcon, palette};

/// 原型给的三个快捷档位。顺序也照它：从保守到放开。
const PRESETS: [u32; 3] = [0o644, 0o755, 0o777];

/// 设好的模式（八进制整数，例如 `0o755`）交回给调用方。
pub type ChmodHandler = Rc<dyn Fn(u32, &mut Window, &mut App)>;

/// chmod 对话框的正文。
pub struct SftpChmodDialog {
    /// 目标条目名，只用于说明"改的是谁"。
    target: SharedString,
    octal: Entity<InputState>,
    on_apply: ChmodHandler,
}

impl SftpChmodDialog {
    /// 弹出对话框。`current` 是对端现在报的权限位（拿不到就是 `None`，
    /// 输入框留空而不是猜一个 644）。
    pub fn open(
        target: impl Into<SharedString>,
        current: Option<u16>,
        on_apply: ChmodHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let target = target.into();
        let initial = current.map(|mode| format!("{mode:o}")).unwrap_or_default();
        let form = cx.new(|cx| Self::new(target, initial, on_apply, window, cx));
        let submit_form = form.clone();
        let blue = palette::blue();

        window.open_dialog(cx, move |dialog, _, cx| {
            let variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));
            let apply = submit_form.clone();

            dialog
                .w(px(420.))
                .title(render_header(cx))
                .child(form.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            Button::new("sftp-chmod-cancel")
                                .ghost()
                                .label("取消")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("sftp-chmod-apply")
                                .custom(variant)
                                .bg(blue)
                                .rounded(cx.theme().radius)
                                .label("应用")
                                .on_click(move |_, window, cx| {
                                    apply.update(cx, |this, cx| this.apply(window, cx));
                                }),
                        ),
                )
        });
    }

    fn new(
        target: SharedString,
        initial: String,
        on_apply: ChmodHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let octal = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("例如 644 或 755")
                .default_value(initial)
        });
        cx.subscribe_in(&octal, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.apply(window, cx);
            }
            cx.notify();
        })
        .detach();

        Self {
            target,
            octal,
            on_apply,
        }
    }

    /// 当前输入解析出来的模式。`None` 时输入框上会画出原因，按钮按不动。
    fn parsed(&self, cx: &App) -> Option<u32> {
        parse_octal(&self.octal.read(cx).value())
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.octal.read(cx).value().to_string();
        let Some(mode) = parse_octal(&raw) else {
            window.push_notification(
                "权限得是 3~4 位八进制数字（每一位不超过 7），例如 644 或 755。",
                cx,
            );
            return;
        };
        let handler = self.on_apply.clone();
        window.close_dialog(cx);
        handler(mode, window, cx);
    }
}

impl Render for SftpChmodDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.parsed(cx);
        // 三个档位是输入框内容的快捷写法，所以选中态按"当前输入是不是这个值"判断，
        // 而不是"上一次点了哪个"：手打了 755，755 那个片也该是亮的。
        let active = mode;

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_baseline()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("目标：")
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().foreground)
                            .child(self.target.clone()),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .children(PRESETS.iter().enumerate().map(|(ix, preset)| {
                        let preset = *preset;
                        let label = SharedString::from(format!("{preset:o}"));
                        let selected = active == Some(preset);
                        let accent = palette::blue();
                        Button::new(("sftp-chmod-preset", ix))
                            .label(label.clone())
                            .font_family(cx.theme().mono_font_family.clone())
                            .small()
                            .when(selected, |button| {
                                button.custom(
                                    ButtonCustomVariant::new(cx)
                                        .color(accent)
                                        .foreground(palette::on_accent())
                                        .hover(accent.lighten(0.08))
                                        .active(accent),
                                )
                            })
                            .when(!selected, |button| {
                                button.secondary().text_color(cx.theme().muted_foreground)
                            })
                            .w_full()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.octal.update(cx, |state, cx| {
                                    state.set_value(label.clone(), window, cx);
                                });
                            }))
                    })),
            )
            .child(
                Input::new(&self.octal)
                    .w_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .cleanable(true),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(match mode {
                        Some(_) => palette::idle(cx),
                        None => cx.theme().danger,
                    })
                    .child(match mode {
                        Some(mode) => SharedString::from(format!(
                            "将对 {} 执行 chmod {:o}（{}）",
                            self.target,
                            mode,
                            describe(mode)
                        )),
                        None => "每一位是 r=4、w=2、x=1，三位分别对应用户、组、其他。".into(),
                    }),
            )
    }
}

/// 把八进制写法读成一个模式。宽松一点：允许前导 0（`0755`）与四位（`4755`）。
fn parse_octal(raw: &str) -> Option<u32> {
    let text = raw.trim();
    // 只认三位（user/group/other）与带特殊位的那四位。`0` 与 `7` 这种一位数不猜：
    // 它既可能是 000 也可能只是没打完。
    if !(3..=4).contains(&text.len()) || !text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mode = u32::from_str_radix(text, 8).ok()?;
    // 八进制只到 7，`from_str_radix` 已经挡住了 8 与 9；这一道是防住四位数越界
    // （`9999` 不是八进制，走不到这儿；`07777` 以上则是越了 setuid 那一位）。
    (mode <= 0o7777).then_some(mode)
}

/// 一句人话说明这串数字放了什么权限。目录档位（`755`）与只读档位（`644`）
/// 是最常点的两个，先说它们，剩下的按位描述。
fn describe(mode: u32) -> &'static str {
    match mode {
        0o444 => "所有人只读",
        0o600 => "只有自己能读写",
        0o644 => "自己能读写，其他人只读",
        0o666 => "所有人可读写",
        0o700 => "只有自己能进、能读写",
        0o755 => "自己能进能读写，其他人只能进和读",
        0o777 => "所有人可读、可写、可执行",
        _ => "按位设置（r=4 w=2 x=1，用户/组/其他）",
    }
}

/// 标题行。盾牌那个图标在原型里是黄色的（配置类）。
fn render_header(cx: &App) -> impl IntoElement {
    let tone: Hsla = palette::idle(cx);
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
                .bg(palette::sky().opacity(0.1))
                .border_1()
                .border_color(palette::sky().opacity(0.3))
                .text_color(palette::sky())
                .child(Icon::new(AppIcon::KeyRound).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(div().text_sm().font_semibold().child("修改权限"))
                .child(
                    div()
                        .text_xs()
                        .text_color(tone)
                        .child("远端会立刻收到这次 setstat，改完列表跟着刷新。"),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::{describe, parse_octal};

    #[test]
    fn octal_accepts_the_ways_people_write_it() {
        assert_eq!(parse_octal("644"), Some(0o644));
        assert_eq!(parse_octal(" 755 "), Some(0o755));
        assert_eq!(parse_octal("0644"), Some(0o644));
        assert_eq!(parse_octal("4755"), Some(0o4755), "setuid 那一档允许写");
        assert_eq!(parse_octal("0"), None, "一位数说不清是哪种档位");
        assert_eq!(parse_octal("77777"), None, "越过 07777");
        assert_eq!(parse_octal("999"), None, "八进制里没有 9");
        assert_eq!(parse_octal("75a"), None);
        assert_eq!(parse_octal(""), None);
    }

    #[test]
    fn the_common_modes_explain_themselves() {
        assert!(describe(0o644).contains("只读"));
        assert!(describe(0o755).contains("进"));
        assert!(describe(0o777).contains("所有人"));
        assert!(describe(0o600).contains("自己"));
        assert!(!describe(0o123).is_empty(), "冷门的也要有一句话");
    }
}
