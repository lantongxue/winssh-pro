//! ZMODEM 确认框：远端 `sz` / `rz` 被认出来之后，请用户给个交代的那一下。
//!
//! 两个形态共用一层壳，正文不一样：
//!
//! - **接收**（远端 `sz`）：文件已经宣布（名字、大小都在），选保存目录后
//!   点"保存"才开始收。默认落在设置里的"默认接收目录"（没配就是系统下载
//!   目录）；同名不加覆盖，落点由 `unique_local_path` 加 `(2)` 让开。
//! - **发送**（远端 `rz`）：说明远端在等，点"选择文件…"唤起系统文件框
//!   （可多选），选完就开传；取消文件框不算反悔，留在框里还能再选。
//!
//! 两个方向的"取消"都会往对端送一串 CAN，让 `rz`/`sz` 收手退出 ——
//! 挂着不管的话，对端要等自己的超时。
//!
//! 自动模式（设置里关掉"接收前先询问"）根本不走这个框，见
//! [`crate::terminal::Emulator::zmodem_handle_offer`]。

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, PathPromptOptions,
    Render, SharedString, Styled as _, Window, div, px,
};

use crate::{
    sftp::human_size,
    terminal::{Emulator, EmulatorEvent},
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 框里那一份状态：方向不同，露出来的东西不一样。
enum Prompt {
    /// 远端 `sz`：文件已宣布，等落点。
    Receive {
        name: String,
        size: Option<u64>,
        dir: PathBuf,
        /// 上一次"保存"失败的原因（目录不可写、名字打不开），红字展示。
        error: Option<SharedString>,
    },
    /// 远端 `rz`：等选文件。
    Send { error: Option<SharedString> },
}

impl Prompt {
    fn is_receive(&self) -> bool {
        matches!(self, Self::Receive { .. })
    }
}

/// 确认框的正文实体。
struct ZmodemTransferBody {
    emulator: Entity<Emulator>,
    prompt: Prompt,
}

impl ZmodemTransferBody {
    /// 取消：告诉对端"别等了"，收起框。
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.emulator.update(cx, |emulator, cx| {
            emulator.zmodem_cancel(cx);
        });
        window.close_dialog(cx);
    }

    /// 保存（接收）：定下落点、让对端开始发。打不开就红字留在框里，
    /// 用户还能换个目录再来 —— 这里不该一错就把整件事收掉。
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Prompt::Receive { dir, error, .. } = &mut self.prompt else {
            return;
        };
        match self.emulator.update(cx, |emulator, cx| {
            emulator.zmodem_accept_save(dir.clone(), cx)
        }) {
            Ok(()) => window.close_dialog(cx),
            Err(failure) => {
                *error = Some(SharedString::from(format!("{failure:#}")));
                cx.notify();
            }
        }
    }

    /// 换保存目录。
    fn change_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("zmodem.save_to").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            _ = this.update_in(cx, |this, _, cx| {
                if let Prompt::Receive {
                    dir: target, error, ..
                } = &mut this.prompt
                {
                    *target = dir;
                    *error = None;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 挑要上传的文件（可多选），挑完就开传。
    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t!("zmodem.choose_files").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            // 三层：通道没断、文件框没出错、用户没取消。任何一层不成立都是
            // "这次没挑"，框留在原地还能再选。
            let Ok(Ok(Some(files))) = picked.await else {
                return;
            };
            if files.is_empty() {
                return;
            }
            _ = this.update_in(cx, |this, window, cx| {
                match this.emulator.update(cx, |emulator, cx| {
                    emulator.zmodem_accept_send(files.clone(), cx)
                }) {
                    Ok(()) => window.close_dialog(cx),
                    Err(failure) => {
                        if let Prompt::Send { error } = &mut this.prompt {
                            *error = Some(SharedString::from(format!("{failure:#}")));
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

impl Render for ZmodemTransferBody {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .p_4()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(match self.prompt {
                        Prompt::Receive { .. } => t!("zmodem.receive_hint").to_string(),
                        Prompt::Send { .. } => t!("zmodem.send_hint").to_string(),
                    })),
            )
            .children(self.render_body(cx))
    }
}

impl ZmodemTransferBody {
    fn render_body(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        match &self.prompt {
            Prompt::Receive {
                name,
                size,
                dir,
                error,
            } => Some(
                v_flex()
                    .gap_2()
                    .child(info_row(
                        t!("zmodem.file_name").to_string(),
                        name.clone(),
                        cx,
                    ))
                    .child(info_row(
                        t!("zmodem.file_size").to_string(),
                        match size {
                            Some(size) => human_size(*size),
                            None => t!("zmodem.size_unknown").to_string(),
                        },
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .w(px(72.))
                                    .child(SharedString::from(t!("zmodem.save_to").to_string())),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(SharedString::from(dir.display().to_string())),
                            )
                            .child(
                                Button::new("zmodem-change-dir")
                                    .ghost()
                                    .small()
                                    .label(t!("zmodem.change_dir"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.change_dir(window, cx);
                                    })),
                            ),
                    )
                    .children(
                        error.clone().map(|error| {
                            div().text_xs().text_color(cx.theme().danger).child(error)
                        }),
                    )
                    .into_any_element(),
            ),
            Prompt::Send { error } => {
                Some(
                    v_flex()
                        .gap_2()
                        .children(error.clone().map(|error| {
                            div().text_xs().text_color(cx.theme().danger).child(error)
                        }))
                        .into_any_element(),
                )
            }
        }
    }
}

/// 一行"标签 + 值"。
fn info_row(label: String, value: String, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .w(px(72.))
                .child(SharedString::from(label)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child(SharedString::from(value)),
        )
}

/// 远端 rz / sz 的确认框。
pub struct ZmodemTransferDialog;

impl ZmodemTransferDialog {
    /// 弹出确认框。提议已经不在了（被别的路径收掉）就什么也不做。
    pub fn open(emulator: Entity<Emulator>, window: &mut Window, cx: &mut App) {
        let Some(offer) = emulator.read(cx).zmodem_offer() else {
            return;
        };
        let prompt = match offer {
            crate::zmodem::Offer::Receive { name, size } => Prompt::Receive {
                name,
                size,
                dir: crate::zmodem::config::global().dir_or_start_dir(),
                error: None,
            },
            crate::zmodem::Offer::Send => Prompt::Send { error: None },
        };
        let body = cx.new(|_| ZmodemTransferBody {
            emulator: emulator.clone(),
            prompt,
        });

        // 会话收场（对端没气了、等决策超时、被别处取消）时把框收掉：提议
        // 已经没了，框上那两颗按钮按下去只会报错。订阅跟着正文实体走，
        // 框一关它就断。
        body.update(cx, |_, cx| {
            cx.subscribe_in(
                &emulator,
                window,
                |_, _, event: &EmulatorEvent, window, cx| {
                    if matches!(event, EmulatorEvent::ZmodemFinished) {
                        window.close_dialog(cx);
                    }
                },
            )
            .detach();
        });

        window.open_dialog(cx, move |dialog, _window, app| {
            let receive = body.read(app).prompt.is_receive();
            let cancel_body = body.clone();
            let primary_body = body.clone();
            let accent = palette::green();
            let primary_variant = ButtonCustomVariant::new(app)
                .color(accent)
                .foreground(palette::on_accent())
                .hover(accent.lighten(0.08))
                .active(accent.darken(0.08));

            dialog
                .w(px(440.))
                // 关掉这一层不能靠点背景：那一下该是"取消"，还是"先别动"？
                // 拿不准的选择不摆给用户 —— 要收它，框里有明确的按钮。
                .overlay_closable(false)
                .title(render_header(receive, app))
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
                            Button::new("zmodem-cancel")
                                .ghost()
                                .label(t!("zmodem.cancel"))
                                .on_click(move |_, window, cx| {
                                    cancel_body.update(cx, |this, cx| {
                                        this.cancel(window, cx);
                                    });
                                }),
                        )
                        .child(
                            Button::new("zmodem-primary")
                                .custom(primary_variant)
                                .bg(accent)
                                .rounded(app.theme().radius)
                                .label(match receive {
                                    true => t!("zmodem.save"),
                                    false => t!("zmodem.choose_files"),
                                })
                                .on_click(move |_, window, cx| {
                                    primary_body.update(cx, |this, cx| match receive {
                                        true => this.save(window, cx),
                                        false => this.pick_files(window, cx),
                                    });
                                }),
                        ),
                )
        });
    }
}

/// 标题行：方向图标 + 标题。
fn render_header(receive: bool, cx: &App) -> impl IntoElement {
    let accent = palette::green();
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
                .bg(accent.opacity(0.1))
                .border_1()
                .border_color(accent.opacity(0.3))
                .text_color(accent)
                .child(
                    Icon::new(match receive {
                        true => AppIcon::Download,
                        false => AppIcon::Upload,
                    })
                    .with_size(px(16.)),
                ),
        )
        .child(
            div()
                .text_sm()
                .font_semibold()
                .child(SharedString::from(match receive {
                    true => t!("zmodem.receive_title").to_string(),
                    false => t!("zmodem.send_title").to_string(),
                })),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Emulator;

    /// 拖一个开着的窗口出来，让对话框层（`Root`）住进去。
    struct Harness {
        #[allow(dead_code)]
        emulator: Entity<Emulator>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn hex_frame(frame_type: u8) -> Vec<u8> {
        let mut bytes = vec![b'*', b'*', 0x18, b'B'];
        bytes.extend_from_slice(format!("{frame_type:02x}").as_bytes());
        bytes.extend_from_slice(b"00000000");
        bytes.extend_from_slice(b"0000\r\n");
        bytes
    }

    /// 两种形态都要真的画得出来：深 builder 链上的错 `cargo check`
    /// 看不见，只有整帧走一遍才掉得出来。
    #[gpui_kit::test]
    fn both_prompts_draw_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        use crate::zmodem::Session;

        cx.update(gpui_kit::init);
        let emulator = cx.update(Emulator::new_empty);
        let (root, cx) = cx.add_window_view({
            let emulator = emulator.clone();
            move |window, cx| {
                gpui_kit::component::Root::new(cx.new(|_| Harness { emulator }), window, cx)
            }
        });
        let _ = &root;

        // 发送形态：一枚 ZRINIT 就够。
        emulator.update(cx, |emulator, cx| {
            emulator.write_output(&hex_frame(0x01), cx);
        });
        cx.update(|window, cx| {
            ZmodemTransferDialog::open(emulator.clone(), window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // 接收形态：真的推一场会话到文件宣布那一刻，提议里带着名字与大小。
        let root =
            std::env::temp_dir().join(format!("winssh-pro-zmodem-dialog-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).expect("建临时目录");
        let source = root.join("report.pdf");
        std::fs::write(&source, vec![0x5au8; 512]).expect("写源文件");
        let mut peer = Session::send(vec![source]).expect("建发送会话");
        peer.step();
        emulator.update(cx, |emulator, cx| {
            emulator.write_output(&peer.take_out(), cx);
        });
        for _ in 0..4000 {
            for chunk in emulator.update(cx, |emulator, _| emulator.take_sent_by_test()) {
                peer.feed(&chunk);
            }
            peer.step();
            let back = peer.take_out();
            if !back.is_empty() {
                emulator.update(cx, |emulator, cx| emulator.write_output(&back, cx));
            }
            if emulator.read_with(cx, |emulator, _| emulator.zmodem_offer().is_some()) {
                break;
            }
        }
        assert!(
            emulator.read_with(cx, |emulator, _| emulator.zmodem_offer().is_some()),
            "该推到文件宣布了"
        );
        cx.update(|window, cx| {
            ZmodemTransferDialog::open(emulator.clone(), window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // 收尾：取消把它收掉，免得任务留在共享的窗口上。
        emulator.update(cx, |emulator, cx| emulator.zmodem_cancel(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        std::fs::remove_dir_all(&root).ok();
    }
}
