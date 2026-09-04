//! 命令面板：标题栏 [`crate::ui::bars::CommandCenterBar`]（看起来像输入框的按钮）
//! 打开的那一层，严格按组件库文档「In a Dialog」模式实现。
//!
//! 触发器为 `Button` 样式组件（模拟输入框外观，`ButtonCustomVariant` 压色），
//! 弹层按文档 In a Dialog 模式 window.open_dialog + dialog.content +
//! Command::new + bordered(false) 等实现，详见
//! <https://gpui-kit.com/docs/components/command>。
//!
//! 一个搜索框加一列命令，分三段：**主机**是设计稿里“搜索连接”的正题，回车就开终端；
//! **视图**切换侧边栏显示哪一块；**动作**是新建主机、快速连接与几处开合。
//!
//! 面板自己没有主机仓储，也不知道窗口是怎么布局的：条目在每次打开时按当前主机现搭，
//! 确认之后只把意图 [`CommandPaletteEvent`] 发出去，由 [`crate::ui::Workspace`] 决定怎么办。
//! 已经有 Action 的那几项（切换侧边栏、事件日志、主机信息）不走这条路 ——
//! 它们直接挂 Action 交给组件派发，快捷键提示因此和真正的绑定同源，不必在这里手写一份。

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Sizable as _, WindowExt as _,
    command::{Command, CommandGroup, CommandItem, CommandState},
    h_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Styled as _, Window, div, px,
};

use crate::{
    host::{Host, HostGroup},
    ui::{
        bars::ActivityAction,
        theme::{AppIcon, palette},
        views::SidebarView,
        workspace::{ToggleRightDock, ToggleSidebar},
    },
};
use rust_i18n::t;

/// 面板宽度，取自设计稿。
const PALETTE_WIDTH: Pixels = px(560.);

/// 面板距窗口顶部的距离。直接盖住标题栏的搜索触发器，
/// 类似 VSCode 的 Command Palette：展开时把背后的输入框遮住，
/// 眼睛和手都不用从入口跑太远。
/// 数值是相对内容区（`window_paddings` 内）的顶部偏移；`20px` 以内的
/// 小值会让弹层上边缘压过触发器（触发器上沿约 `5px`），从而完全遮住它。
/// 配合对话框自带的 `slide-down` 动画，看起来就是从顶部输入框长出来的。
const PALETTE_MARGIN_TOP: Pixels = px(4.);

/// 列表最多这么高，再多就滚动。
const PALETTE_MAX_HEIGHT: Pixels = px(360.);

/// 行内图标的尺寸，与侧边栏保持一致。
const ICON_SIZE: Pixels = px(16.);

/// 面板确认一条命令之后往外发的请求。
///
/// 面板只说“要做这件事”，做不做得成、怎么做，是宿主的事。
#[derive(Clone, Debug)]
pub enum CommandPaletteEvent {
    /// 打开这台主机的会话。
    OpenHost(Host),
    /// 侧边栏切到这个视图。
    ShowView(SidebarView),
    /// 新建一台主机。
    NewHost,
    /// 往保险箱里新建一条凭据。
    NewCredential,
    /// 在一扇独立窗口里全屏打开保险箱。
    OpenVaultWindow,
    /// 活动栏底部那几个全局动作里的一个。
    Activity(ActivityAction),
}

/// 面板里的一条命令：条目长什么样，加上确认之后要做的事。
///
/// `event` 为空表示这条命令自带 Action —— 组件在确认时已经派发过了，
/// 面板这边只需要收起自己。
struct PaletteCommand {
    item: CommandItem,
    event: Option<CommandPaletteEvent>,
}

impl PaletteCommand {
    fn new(item: CommandItem, event: CommandPaletteEvent) -> Self {
        Self {
            item,
            event: Some(event),
        }
    }

    /// 一条只派发 Action 的命令。
    fn dispatching(item: CommandItem) -> Self {
        Self { item, event: None }
    }
}

/// 面板里的一段。段的顺序就是 [`CommandState`] 给出的 `section` 下标，
/// 所以这个列表的顺序必须和渲染时喂给 [`Command`] 的顺序一致。
struct PaletteSection {
    label: SharedString,
    commands: Vec<PaletteCommand>,
}

pub struct CommandPaletteDialog {
    state: Entity<CommandState>,
    sections: Vec<PaletteSection>,
    /// 面板是不是正开着。快捷键再按一次是收起，这个标记就是那一下的依据 ——
    /// Esc、点遮罩和确认命令三条路都会把它放下来。
    open: bool,
}

impl CommandPaletteDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            state: cx.new(|cx| CommandState::new(window, cx)),
            sections: Vec::new(),
            open: false,
        }
    }

    /// 弹出命令面板；已经开着就收起 —— 同一个快捷键既是“打开”也是“算了”。
    ///
    /// `hosts` 是当前的主机分组，条目按它现搭：面板不持有主机，也就不会拿着一份旧的。
    pub fn open(this: &Entity<Self>, hosts: &[HostGroup], window: &mut Window, cx: &mut App) {
        if this.read(cx).open {
            this.update(cx, |this, _| this.open = false);
            window.close_dialog(cx);
            return;
        }

        let state = this.update(cx, |this, cx| {
            this.sections = build_sections(hosts);
            this.open = true;
            cx.notify();
            this.state.clone()
        });

        let content = this.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let closed = content.clone();
            let content_inner = content.clone();
            dialog
                // 按文档「In a Dialog」：`close_button(false) + p_0 + content(|c| c.child(Command))`
                // `Command` 内部已有搜索框与列表的边距，对话框本身不再留白。
                // `bordered(false)` 在 `render_command` 中设置，对话框的圆角与边框即为最终外框。
                .close_button(false)
                .p_0()
                .w(PALETTE_WIDTH)
                .margin_top(PALETTE_MARGIN_TOP)
                .content(move |content, _, _| content.child(content_inner.clone()))
                // Esc 或点遮罩关掉的时候，面板自己并不知道；标记在这里同步回去。
                // `Command` 的 `on_cancel` 会先清非空 query 再传播 `Cancel`，
                // 真正的关闭由 `Dialog` 负责，此处仅同步 `open` 标记。
                .on_close(move |_, _, cx| {
                    closed.update(cx, |this, _| this.open = false);
                })
        });

        // 打开就落在搜索框里：这一层的第一件事是打字。
        state.update(cx, |state, cx| {
            state.set_query("", window, cx);
            state.focus(window, cx);
        });
    }

    /// 确认了第 `index` 条命令。带 Action 的那几条组件已经派发过，这里只管收起面板；
    /// 其余的把意图发出去，让宿主接手。
    ///
    /// 先收起再发：新建主机会紧接着弹另一个对话框，反过来就把新弹的那个关掉了。
    fn confirm(&mut self, index: IndexPath, window: &mut Window, cx: &mut Context<Self>) {
        let event = self
            .sections
            .get(index.section)
            .and_then(|section| section.commands.get(index.row))
            .and_then(|command| command.event.clone());

        self.open = false;
        window.close_dialog(cx);

        if let Some(event) = event {
            cx.emit(event);
        }
    }

    fn render_command(&self, cx: &mut Context<Self>) -> Command {
        let this = cx.entity().downgrade();

        self.sections.iter().fold(
            Command::new(&self.state)
                // 对话框已经是一层框了，面板不必再画一圈。
                .bordered(false)
                // 和标题栏那个触发器上的字一样，点进来之后说的还是同一件事。
                .placeholder(t!("command_center.search"))
                .max_h(PALETTE_MAX_HEIGHT)
                .empty(|_, _, cx| {
                    div()
                        .py_6()
                        .w_full()
                        .text_center()
                        .text_sm()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(t!("palette.empty")))
                })
                // 面板全靠键盘用，底下这行是它的说明书。
                .footer(|_, _, cx| {
                    h_flex()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(t!("palette.hint_select")))
                        .child(SharedString::from(t!("palette.hint_run")))
                        .child(SharedString::from(t!("palette.hint_close")))
                })
                .on_confirm(move |index, window, cx| {
                    _ = this.update(cx, |this, cx| this.confirm(index, window, cx));
                }),
            |command, section| {
                command.separator().group(
                    CommandGroup::new()
                        .label(section.label.clone())
                        .items(section.commands.iter().map(|command| command.item.clone())),
                )
            },
        )
    }
}

/// 按当前主机搭出面板的三段。顺序即 `section` 下标，改这里也要改
/// [`CommandPaletteDialog::render_command`] 里喂进去的顺序。
fn build_sections(hosts: &[HostGroup]) -> Vec<PaletteSection> {
    vec![
        PaletteSection {
            label: t!("palette.section_hosts").into(),
            commands: host_commands(hosts),
        },
        PaletteSection {
            label: t!("palette.section_views").into(),
            commands: view_commands(),
        },
        PaletteSection {
            label: t!("palette.section_actions").into(),
            commands: action_commands(),
        },
    ]
}

/// 每台主机一条命令。行是自己画的：名字之外还带上地址 ——
/// 同名主机分在不同分组里是常事，光看名字选不出是哪一台。
fn host_commands(groups: &[HostGroup]) -> Vec<PaletteCommand> {
    groups
        .iter()
        .flat_map(|group| {
            group.hosts.iter().map(move |host| {
                let name = host.name.clone();
                let address = SharedString::from(host.address());
                let row_name = name.clone();
                let row_address = address.clone();

                PaletteCommand::new(
                    CommandItem::new()
                        .label(name)
                        .keywords([address, group.name.clone(), host.platform_tag()])
                        .child(move |_, cx| {
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .child(
                                    Icon::new(AppIcon::Server)
                                        .with_size(ICON_SIZE)
                                        .flex_shrink_0()
                                        .text_color(palette::teal()),
                                )
                                .child(div().flex_1().min_w_0().truncate().child(row_name.clone()))
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .font_family("monospace")
                                        .text_xs()
                                        .text_color(palette::idle(cx))
                                        .child(row_address.clone()),
                                )
                        }),
                    CommandPaletteEvent::OpenHost(host.clone()),
                )
            })
        })
        .collect()
}

/// 侧边栏的四个视图各一条。
fn view_commands() -> Vec<PaletteCommand> {
    SidebarView::ALL
        .into_iter()
        .map(|view| {
            PaletteCommand::new(
                CommandItem::new()
                    .label(view.label())
                    .icon(view.icon())
                    .keywords([view.id(), "view", "视图"]),
                CommandPaletteEvent::ShowView(view),
            )
        })
        .collect()
}

/// 全局动作。前三条这个应用还没有 Action，走事件；后三条已经有绑定，
/// 挂 Action 交给组件派发，行尾那个键位提示也就跟着来了。
fn action_commands() -> Vec<PaletteCommand> {
    vec![
        PaletteCommand::new(
            CommandItem::new()
                .label(t!("palette.new_host"))
                .icon(IconName::Plus)
                .keywords(["new", "host", "add"]),
            CommandPaletteEvent::NewHost,
        ),
        PaletteCommand::new(
            CommandItem::new()
                .label(t!("palette.new_credential"))
                .icon(AppIcon::ShieldCheck)
                .keywords(["new", "credential", "password", "key", "vault"]),
            CommandPaletteEvent::NewCredential,
        ),
        PaletteCommand::new(
            CommandItem::new()
                .label(t!("palette.open_vault_window"))
                .icon(IconName::Maximize)
                .keywords(["vault", "window", "password", "保险箱", "窗口"]),
            CommandPaletteEvent::OpenVaultWindow,
        ),
        PaletteCommand::new(
            CommandItem::new()
                .label(ActivityAction::QuickConnect.label())
                .icon(AppIcon::Zap)
                .keywords(["quick", "connect", "ssh"]),
            CommandPaletteEvent::Activity(ActivityAction::QuickConnect),
        ),
        PaletteCommand::new(
            CommandItem::new()
                .label(ActivityAction::Assistant.label())
                .icon(AppIcon::Sparkles)
                .keywords(["ai", "assistant", "copilot"]),
            CommandPaletteEvent::Activity(ActivityAction::Assistant),
        ),
        PaletteCommand::dispatching(
            CommandItem::new()
                .label(t!("palette.toggle_sidebar"))
                .icon(IconName::PanelLeft)
                .keywords(["sidebar", "panel"])
                .action(Box::new(ToggleSidebar)),
        ),
        PaletteCommand::dispatching(
            CommandItem::new()
                .label(t!("palette.toggle_right_dock"))
                .icon(IconName::PanelRight)
                .keywords(["info", "settings", "right"])
                .action(Box::new(ToggleRightDock)),
        ),
    ]
}

impl EventEmitter<CommandPaletteEvent> for CommandPaletteDialog {}

impl Render for CommandPaletteDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_command(cx)
    }
}
