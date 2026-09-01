//! 远端条目命名对话框：新建文件、新建目录、重命名共用的一只输入框。
//!
//! 对应原型 `SessionBottomSFTPPanel` 里那三个长得几乎一样的模态框
//! （"Create New Remote File" / "Create New Remote Directory" / "Rename Item"）。
//! 它们的差别只有标题、图标、提示文案和按钮文字，采集的东西都是**一个目录内的
//! 条目名**，所以合成一个组件：形态在这儿，落到远端做什么由调用方决定
//! （[`Self::on_submit`] 拿到的是校验过的名字）。
//!
//! 这里刻意不碰 [`crate::service::SftpService`]：路径拼接、"是不是已经存在"、
//! 建完之后要不要顺手打开编辑器，都是面板的事。对话框只保证交出去的名字
//! 是一个能用的条目名 —— 这是它自己就能判断、而且判断错了会写坏远端的那部分。

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Colorize as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};

use crate::ui::theme::{AppIcon, palette};

/// 这一只对话框在问哪件事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SftpFormKind {
    /// 在当前远端目录里新建一个空文件。
    NewFile,
    /// 在当前远端目录里新建一个子目录。
    NewFolder,
    /// 把已有的条目改名。
    Rename,
}

impl SftpFormKind {
    fn title(self) -> &'static str {
        match self {
            Self::NewFile => "新建远端文件",
            Self::NewFolder => "新建远端目录",
            Self::Rename => "重命名条目",
        }
    }

    /// 输入框上面那行说明。原型把"在哪个目录里建"写进标签，
    /// 因为面板可能已经翻到别处去了 —— 这一行就是落点。
    fn label(self, subject: &SharedString) -> SharedString {
        match self {
            Self::NewFile => SharedString::from(format!("{subject} 内的文件名")),
            Self::NewFolder => SharedString::from(format!("{subject} 内的目录名")),
            Self::Rename => SharedString::from(format!("{subject} 的新名称")),
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::NewFile => "例如 app.config.json 或 script.sh",
            Self::NewFolder => "例如 backups 或 releases",
            Self::Rename => "",
        }
    }

    fn ok_text(self) -> &'static str {
        match self {
            // 新建文件之后紧接着打开编辑器（原型就是 "Create & Edit"）：
            // 建一个空文件不写点东西，等于没建完。
            Self::NewFile => "创建并编辑",
            Self::NewFolder => "创建目录",
            Self::Rename => "保存",
        }
    }

    fn icon(self) -> AppIcon {
        match self {
            Self::NewFile => AppIcon::FilePlus,
            Self::NewFolder => AppIcon::FolderPlus,
            Self::Rename => AppIcon::Pencil,
        }
    }

    /// 图标颜色跟着语义走：新建文件是"加东西"（青），目录是橙色（和表格里的
    /// 文件夹同一个颜色），改名属于改配置（蓝）。
    fn tone(self) -> gpui::Hsla {
        match self {
            Self::NewFile => palette::teal(),
            Self::NewFolder => palette::coral(),
            Self::Rename => palette::sky(),
        }
    }
}

/// 校验通过之后要干的事。参数是去掉首尾空格、确认过能用的条目名。
pub type SftpNameHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

/// 命名对话框的正文。外壳（宽度、标题、底部两个按钮）由 [`Self::open`] 现搭。
pub struct SftpFormDialog {
    kind: SftpFormKind,
    /// 这个名字要落在哪儿：新建时是目标目录，改名时是原条目名。
    /// 只用来写标签，不参与拼接 —— 拼接由面板做，它知道完整路径。
    subject: SharedString,
    name: Entity<InputState>,
    on_submit: SftpNameHandler,
}

impl SftpFormDialog {
    /// 弹出对话框。
    ///
    /// - `directory`：新建时的目标目录（改名时不用，传原条目名即可，标签按 kind 走）；
    /// - `initial`：预填的内容，只有改名用得上（原型改名框是预填的）；
    /// - `on_submit`：校验通过后调用，对话框已经收起。
    pub fn open(
        kind: SftpFormKind,
        directory: impl Into<SharedString>,
        initial: impl Into<SharedString>,
        on_submit: SftpNameHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let subject = match kind {
            // 改名问的是"这个条目叫什么"，新建问的是"落在哪个目录"。
            SftpFormKind::Rename => initial.into(),
            SftpFormKind::NewFile | SftpFormKind::NewFolder => directory.into(),
        };
        let initial = match kind {
            SftpFormKind::Rename => subject.clone(),
            _ => SharedString::default(),
        };
        let form = cx.new(|cx| Self::new(kind, subject, initial, on_submit, window, cx));
        let submit_form = form.clone();
        let blue = palette::blue();

        window.open_dialog(cx, move |dialog, _, cx| {
            let variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));
            let submit = submit_form.clone();

            dialog
                .w(px(420.))
                .title(render_header(kind, cx))
                .child(form.clone())
                .footer(
                    v_flex()
                        .w_full()
                        .gap_3()
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette::idle(cx))
                                .child(hint(kind)),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .justify_end()
                                .gap_2()
                                .pt_3()
                                .border_t_1()
                                .border_color(cx.theme().border)
                                .child(
                                    Button::new("sftp-form-cancel")
                                        .ghost()
                                        .label("取消")
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("sftp-form-ok")
                                        .custom(variant)
                                        .bg(blue)
                                        .rounded(cx.theme().radius)
                                        .label(kind.ok_text())
                                        .on_click(move |_, window, cx| {
                                            submit.update(cx, |this, cx| this.submit(window, cx));
                                        }),
                                ),
                        ),
                )
        });
    }

    fn new(
        kind: SftpFormKind,
        subject: SharedString,
        initial: SharedString,
        on_submit: SftpNameHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder(kind.placeholder());
            match initial.is_empty() {
                true => state,
                false => state.default_value(initial),
            }
        });
        // 回车等价于点主按钮：改名这种事基本都是键盘活。
        cx.subscribe_in(&name, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit(window, cx);
            }
        })
        .detach();

        Self {
            kind,
            subject,
            name,
            on_submit,
        }
    }

    /// 校验 + 收壳 + 交出去。
    ///
    /// 顺序是有讲究的：先收起再回调，这样回调里接着开一只新对话框
    /// （新建文件之后打开编辑器就是）不会被这一下关掉。
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.name.read(cx).value().to_string();
        let name = match validate_name(&raw) {
            Ok(name) => name,
            Err(message) => {
                window.push_notification(message, cx);
                return;
            }
        };
        let handler = self.on_submit.clone();
        window.close_dialog(cx);
        handler(name, window, cx);
    }
}

impl Render for SftpFormDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_baseline()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.kind.label(&self.subject))
                    // 目标目录长在窄屏上会顶破对话框，所以让它自己截断。
                    .when(self.kind != SftpFormKind::Rename, |this| {
                        this.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_color(cx.theme().foreground)
                                .child(self.subject.clone()),
                        )
                    }),
            )
            .child(Input::new(&self.name).w_full())
    }
}

/// 标签下面那行小字：说清楚这个名字会被怎么处理。
fn hint(kind: SftpFormKind) -> &'static str {
    match kind {
        SftpFormKind::NewFile => "新建的是空文件，创建完直接进编辑器。同名文件已存在时不会覆盖。",
        SftpFormKind::NewFolder => "只在当前这一层建目录，不会顺路补出中间层级。",
        // 这一版协议的 rename 报文里没有覆盖标志，对端遇到同名条目会直接拒绝，
        // 所以这里说"不会顶掉"——顶掉旧名字的那条路是上传，不是改名。
        SftpFormKind::Rename => {
            "重命名只在当前这一层生效；新名字已经有同名条目时，对端会拒绝这次改名。"
        }
    }
}

/// 一个目录内的条目名。
///
/// 三条规则都是"写错了会动到别的东西"级别的问题，所以宁可在这儿挡住：
/// 带 `/` 的名字会让拼接出来的路径跑到别的目录去；`.` 与 `..` 指的是目录本身
/// 和上级，拿它们当条目名去 rename 或删除，改的是别人的目录。
fn validate_name(raw: &str) -> Result<SharedString, SharedString> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("条目名不能是空的。".into());
    }
    if name.contains('/') {
        return Err("条目名里不能有 `/`：这里改的只是一个目录内的名字。".into());
    }
    if name == "." || name == ".." {
        return Err("`/.` 和 `..` 指的是目录本身与上级，不能当条目名。".into());
    }
    Ok(SharedString::from(name))
}

/// 标题行：一个带色的小方块 + 标题，跟主机表单同一套排法。
fn render_header(kind: SftpFormKind, cx: &App) -> impl IntoElement {
    let tone = kind.tone();
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
                .bg(tone.opacity(0.1))
                .border_1()
                .border_color(tone.opacity(0.3))
                .text_color(tone)
                .child(Icon::new(kind.icon()).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(div().text_sm().font_semibold().child(kind.title()))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("这一步会直接作用在远端的文件系统上。"),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::validate_name;

    #[test]
    fn a_name_is_one_entry_within_one_directory() {
        assert_eq!(validate_name("  app.conf  ").unwrap(), "app.conf");
        assert_eq!(validate_name("app.tar.gz").unwrap(), "app.tar.gz");
        assert_eq!(validate_name(".bashrc").unwrap(), ".bashrc");
        assert!(validate_name("   ").is_err(), "空名字不该发去远端");
        assert!(validate_name("a/b").is_err(), "带斜杠会跑到别的目录去");
        assert!(validate_name(".").is_err());
        assert!(validate_name("..").is_err());
    }
}
