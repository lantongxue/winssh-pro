//! 一台主机的命令历史。
//!
//! 列的是**这台机器**上跑过的命令，不是这条会话跑过的 —— 历史跟着主机走
//! （见 [`crate::command_history`]），所以关掉标签页再连回来，上次那些还在，
//! 同一台机器开着两条会话时看到的也是同一份。
//!
//! 一行一条：跑成了没有、命令本身、什么时候跑的。点一行就把那条命令发回当前
//! 这条会话的终端里 —— 这一层最常见的用法就是"上次那条长命令是怎么写的"。
//! 行尾另有一颗删除，悬停才显形：一条历史留不留得住是用户的事，不必为了去掉
//! 其中一行把一整台机器的历史都清了。
//!
//! 数据是打开这一刻从库里取的一份快照（最多 [`HISTORY_LIMIT`] 条），搜索在这份
//! 快照上过滤，不回库里查：一台主机的历史就这么多，两百毫秒一次的往返换不来
//! 什么，而输入框每敲一个字都发一次查询才是真的卡。
//!
//! # 底下那个开关
//!
//! 记不到命令最常见的原因不是这一层坏了，而是**对端 shell 根本没发标记**。
//! 所以这扇窗除了"看历史"还担一件事：把那个开关摆在用户发现问题的地方 ——
//! 打开它，每次连上这台机器就替他往对端装一份 OSC 133 集成
//! （[`crate::terminal::shell_integration`]）。
//!
//! 开关是"扳一下要先点头"的那一类：我们要往用户的服务器上打一行字，所以扳到
//! 开之前先弹一层，把**那一行原样**摆出来给他看。关的时候不弹 —— 停手不需要
//! 许可，而且已经装进当前这条 shell 的那一份也收不回来（重连才干净）。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, VirtualListScrollHandle,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    switch::Switch,
    v_flex, v_virtual_list,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px, size,
};

use crate::{
    command_history::CommandRecord,
    host::Host,
    service::{CommandHistoryService, command_history_service::HISTORY_LIMIT},
    terminal::shell_integration,
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 一行有多高。定高才能走虚拟列表 —— 五百行都靠 flex 量尺的话，
/// 光是打开这一层就要把一帧的预算吃光（见 AGENTS.md 的渲染性能那一节）。
const ROW_HEIGHT: Pixels = px(30.);

/// 列表区的高度。固定值：行数从零到五百都有，跟着内容长的话，
/// 这扇窗会在"两行高"和"顶出屏幕"之间反复横跳。
const LIST_HEIGHT: Pixels = px(320.);

/// 退出码那一格的宽度，够放下三位数加一个符号。
const STATUS_WIDTH: Pixels = px(34.);

const ICON: Pixels = px(14.);

/// 行尾那颗删除的图标大小。比脚注那颗「清空」小一号：一条和一整台不是同一件事，
/// 分量也该看得出来。
const ROW_ICON: Pixels = px(12.);

/// 每一行自己是一个悬停组，行尾那颗删除认它显形。
const ROW_GROUP: &str = "command-history-row";

/// 确认框里那块脚本最多显示多高，超出的部分滚动。
///
/// 定高而不是跟着内容长：[`shell_integration::SCRIPT`] 是一整行八百来字，
/// 换行之后有十来行，让它顶着长会把确认框撑得比屏幕还高，
/// 而底下那两颗按钮正是这一层要用户按的。
const SCRIPT_MAX_HEIGHT: Pixels = px(200.);

/// 点中一条历史之后把命令交给谁。宿主（终端标签页）拿它送进当前会话。
pub type HistoryRunHandler = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// 扳动 shell 集成开关之后把新值交给谁。
///
/// 宿主（终端标签页）拿它做两件这一层够不着的事：写回主机那一行，
/// 以及在扳到开的时候立刻往当前这条会话里装一份 —— 用户扳开关是想**现在**
/// 就开始记，不是下次连上才开始。
pub type ShellIntegrationHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

pub struct CommandHistoryDialog {
    host_id: SharedString,
    service: CommandHistoryService,
    search: Entity<InputState>,
    /// 打开这一刻从库里取的快照，最近跑的在最前。
    records: Vec<CommandRecord>,
    /// 过了搜索之后还剩哪几条，存的是 [`Self::records`] 里的下标。
    visible: Vec<usize>,
    /// 虚拟列表的行高表，长度必须和 [`Self::visible`] 一致。
    row_sizes: Rc<Vec<Size<Pixels>>>,
    scroll: VirtualListScrollHandle,
    on_run: HistoryRunHandler,
    /// 这台主机上那个 shell 集成开关现在是开是关。
    ///
    /// 界面持有的一份快照，正本在 `hosts` 表那一行上（见
    /// [`crate::host::Host::shell_integration`]）。扳一下先改这里再落库，
    /// 和别处一样：用户按下去的那一下该立刻看得见。
    shell_integration: bool,
    on_shell_integration: ShellIntegrationHandler,
    /// 还没读完库。读完之前列表是空的，但那不等于"没有历史"，
    /// 空状态那句话得分得清这两者。
    loading: bool,
    _search_sub: Subscription,
}

impl CommandHistoryDialog {
    /// 弹出这台主机的命令历史。
    pub fn open(
        host: Host,
        service: CommandHistoryService,
        on_run: HistoryRunHandler,
        on_shell_integration: ShellIntegrationHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let dialog_view = cx.new(|cx| {
            Self::new(
                host.clone(),
                service,
                on_run,
                on_shell_integration,
                window,
                cx,
            )
        });
        // 读库在这儿起，不在 `new` 里：构造函数不做 I/O，测试才能拿一份现成的
        // 快照把这一层画出来（gpui 的确定性测试容不下一次跨线程的唤醒）。
        dialog_view.update(cx, |this, cx| this.load(cx));
        let clear_view = dialog_view.clone();

        window.open_dialog(cx, move |dialog, _, cx| {
            let clear_view = clear_view.clone();
            dialog
                .w(px(640.))
                .title(render_header(&host, cx))
                .child(dialog_view.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            Button::new("command-history-clear")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(AppIcon::Trash).with_size(ICON))
                                .label(t!("command_history.clear"))
                                .on_click(move |_, window, cx| {
                                    clear_view.update(cx, |this, cx| this.clear(window, cx));
                                }),
                        )
                        .child(
                            Button::new("command-history-close")
                                .ghost()
                                .label(t!("command_history.close"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        ),
                )
        });
    }

    fn new(
        host: Host,
        service: CommandHistoryService,
        on_run: HistoryRunHandler,
        on_shell_integration: ShellIntegrationHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("command_history.search_placeholder"))
        });
        let _search_sub =
            cx.subscribe_in(&search, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.apply_filter(cx);
                }
            });

        Self {
            host_id: host.id.clone(),
            service,
            search,
            records: Vec::new(),
            visible: Vec::new(),
            row_sizes: Rc::new(Vec::new()),
            scroll: VirtualListScrollHandle::new(),
            on_run,
            shell_integration: host.shell_integration,
            on_shell_integration,
            loading: true,
            _search_sub,
        }
    }

    /// 从库里取一份快照。读不出来只记日志、当成空的 —— 这一层不是会话的
    /// 关键路径，为它弹一个错误框打断用户不值当。
    fn load(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let host_id = self.host_id.to_string();
        cx.spawn(async move |this, cx| {
            let records = service.load_for_host(&host_id).await.unwrap_or_else(|error| {
                log::warn!("命令历史读不出来：{error:#}");
                Vec::new()
            });
            this.update(cx, |this, cx| {
                this.records = records;
                this.loading = false;
                this.apply_filter(cx);
            })
            .ok();
        })
        .detach();
    }

    /// 按搜索框里的字重算一遍还剩哪几条。
    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().trim().to_lowercase();
        self.visible = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                query.is_empty() || record.command.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect();
        // 行高表跟着行数走：两者不一致时虚拟列表算出来的可见区间就是错的。
        self.row_sizes = Rc::new(vec![size(px(0.), ROW_HEIGHT); self.visible.len()]);
        cx.notify();
    }

    /// 点了某一行：把那条命令发回当前会话，并收起这一层。
    fn run(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self.record_at(row).map(|record| record.command.to_string()) else {
            return;
        };
        let on_run = self.on_run.clone();
        window.close_dialog(cx);
        on_run(command, window, cx);
    }

    /// 用户扳了 shell 集成那个开关。
    ///
    /// 两个方向不对称，这是故意的：
    ///
    /// - **扳到开**先弹一层确认，把要送出去的那一行原样摆出来。开关这时候
    ///   **不动** —— 它显示的是"这台主机现在是什么样"，而用户还没点头；
    ///   提前扳过去等于先斩后奏，取消掉还得再扳回来。
    /// - **扳到关**立刻生效。停手不需要许可。
    fn toggle_shell_integration(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if on {
            self.confirm_shell_integration(window, cx);
        } else {
            self.apply_shell_integration(false, window, cx);
        }
    }

    /// 把新值记下来，并交给宿主去落库、去装。
    fn apply_shell_integration(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.shell_integration = on;
        cx.notify();
        (self.on_shell_integration.clone())(on, window, cx);
    }

    /// 开之前那一层：把 [`shell_integration::SCRIPT`] 整段摆出来，用户点头才装。
    ///
    /// 这一层不是走过场。我们要做的事是"往对端 shell 里打一行字"，而对端是
    /// 用户的生产服务器 —— 他有权在这之前看清楚打的是什么，也有权把它复制走
    /// 自己研究。所以脚本原样显示（不是摘要、不是省略号），还配一颗复制。
    fn confirm_shell_integration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let dialog = dialog.clone();
            alert
                .icon(
                    Icon::new(AppIcon::Terminal)
                        .with_size(px(18.))
                        .text_color(palette::amber()),
                )
                .title(t!("shell_integration.confirm_title"))
                .description(SharedString::from(t!("shell_integration.confirm_body")))
                .width(px(640.))
                .child(render_script(cx))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("shell_integration.confirm_ok"))
                        .cancel_text(t!("shell_integration.confirm_cancel"))
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    dialog
                        .update(cx, |this, cx| {
                            this.apply_shell_integration(true, window, cx)
                        })
                        .ok();
                    // 返回真：确认框收起，底下那扇命令历史还开着。
                    true
                })
        });
    }

    /// 删掉一条历史。界面先少一行，落库随后 —— 和「清空」一个规矩：
    /// 用户按下去的那一下该立刻看得见。
    ///
    /// 删的是**库里那一行**（拿 [`CommandRecord::id`]），不是"列表的第几条"：
    /// 列表是过了搜索的一份子集，搜着的时候行号和库里对不上。
    fn delete(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(index) = self.visible.get(row).copied() else {
            return;
        };
        let id = self.records.remove(index).id.to_string();
        self.apply_filter(cx);

        let service = self.service.clone();
        cx.spawn(async move |_, _| {
            if let Err(error) = service.delete(&id).await {
                log::warn!("命令历史删不掉：{error:#}");
            }
        })
        .detach();
    }

    /// 清空这台主机的历史。界面先空掉，落库随后 —— 顺序和别处一致：
    /// 用户按下去的那一下该立刻看得见。
    fn clear(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.records.clear();
        self.apply_filter(cx);

        let service = self.service.clone();
        let host_id = self.host_id.to_string();
        cx.spawn(async move |_, _| {
            if let Err(error) = service.clear_host(&host_id).await {
                log::warn!("命令历史清不掉：{error:#}");
            }
        })
        .detach();
    }

    fn record_at(&self, row: usize) -> Option<&CommandRecord> {
        self.visible
            .get(row)
            .and_then(|index| self.records.get(*index))
    }

    fn render_row(&self, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(record) = self.record_at(row) else {
            return div().h(ROW_HEIGHT).into_any_element();
        };
        // "成了没有"的口径归领域层（[`CommandRecord::succeeded`]），这里只挑字形。
        let (status, tone) = match (record.succeeded(), record.exit_code) {
            // 跑成了不必写 0：一个对勾就够，那一列是用来一眼扫过去的。
            (Some(true), _) => (SharedString::from("✓"), palette::green()),
            // 失败则写出退出码本身：`1` 和 `127`（命令根本不存在）是两回事。
            (Some(false), Some(code)) => (SharedString::from(code.to_string()), cx.theme().danger),
            // 没拿到退出码就照实说不知道，不能画成成功。
            _ => (SharedString::from("—"), palette::idle(cx)),
        };

        h_flex()
            .id(("command-history-row", row))
            .h(ROW_HEIGHT)
            .w_full()
            .px_2()
            .gap_2()
            .items_center()
            .cursor_pointer()
            .group(ROW_GROUP)
            .hover(|this| this.bg(palette::selected_row(cx)))
            .on_click(cx.listener(move |this, _, window, cx| this.run(row, window, cx)))
            .child(
                div()
                    .w(STATUS_WIDTH)
                    .flex_shrink_0()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(tone)
                    .child(status),
            )
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(cx.theme().foreground)
                    .child(record.command.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(record.executed_at.clone()),
            )
            .child(self.render_row_delete(row, cx))
            .into_any_element()
    }

    /// 行尾那颗删除：悬停在这一行上才显形，平时隐着但占着位。
    ///
    /// 和主机分组那一颗同一套（见 [`crate::ui::views::HostExplorerView`]）——
    /// 隐着的时候点不到（`invisible` 的元素不参与命中测试），时间那一列也不会
    /// 因为它来回出现左右挪。
    ///
    /// 删一条不问。这一层的分量和「清空」不一样：清空是一台机器的历史全没，
    /// 而这里去掉的是列表上的一行，代价小到不值得拦一道 —— 何况用户按它的
    /// 常见理由正是"这行有不该留的东西，现在就拿掉"。
    fn render_row_delete(&self, row: usize, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new(("command-history-delete", row))
            .ghost()
            .xsmall()
            .icon(
                Icon::new(AppIcon::Trash)
                    .with_size(ROW_ICON)
                    .text_color(cx.theme().danger),
            )
            .tooltip(t!("command_history.delete"))
            .accessibility_id(t!("command_history.delete"))
            .invisible()
            .group_hover(ROW_GROUP, |style| style.visible())
            .on_click(cx.listener(move |this, _, _, cx| {
                // 这一击只删这一条，不再冒泡成"把它发进会话"。
                cx.stop_propagation();
                this.delete(row, cx);
            }))
    }

    /// 列表空着的时候那句话。读库还没回来、这台机器确实没有历史、
    /// 搜索没搜着 —— 三件事得分开说。
    fn empty_note(&self) -> SharedString {
        if self.loading {
            return t!("command_history.loading").into();
        }
        if self.records.is_empty() {
            return t!("command_history.empty").into();
        }
        t!("command_history.no_match").into()
    }

    /// 底下那一块：一句提示，加上 shell 集成那个开关。
    ///
    /// 开关摆在这儿而不是主机表单里，是因为**这里才是用户发现问题的地方** ——
    /// 列表空着、那句空状态说"对端 shell 得自己打标记"，下一眼就该看见
    /// "那你替我装上"这个开关。
    fn render_shell_integration(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.shell_integration;
        // 开着和关着说的不是同一件事：关着要讲"打开会发生什么"，
        // 开着要讲"已经在做什么、以及当前这条会话的处境"。
        let note = if on {
            t!("shell_integration.note_on")
        } else {
            t!("shell_integration.note_off")
        };

        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_3()
            .p_2p5()
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(SharedString::from(t!("shell_integration.label"))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(note)),
                    ),
            )
            .child(
                Switch::new("command-history-shell-integration")
                    .checked(on)
                    .accessibility_label(t!("shell_integration.label"))
                    .on_change(cx.listener(|this, on: &bool, window, cx| {
                        this.toggle_shell_integration(*on, window, cx);
                    })),
            )
    }
}

/// 确认框里那块脚本：原样一整行，等宽字，能滚，还配一颗复制。
///
/// 显示的就是 [`shell_integration::SCRIPT`] 这个常量本身，和送出去的字节
/// 逐字相同 —— 展示一份、发出去另一份是这一层最不能犯的错。
fn render_script(cx: &App) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_1p5()
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(t!("shell_integration.script_caption"))),
                )
                .child(
                    Button::new("shell-integration-copy")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Copy)
                        .label(t!("shell_integration.copy"))
                        .on_click(|_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                shell_integration::SCRIPT.to_owned(),
                            ));
                        }),
                ),
        )
        .child(
            div()
                .id("shell-integration-script")
                .w_full()
                .max_h(SCRIPT_MAX_HEIGHT)
                .overflow_y_scroll()
                .p_2()
                .rounded(cx.theme().radius)
                .bg(palette::recessed(cx))
                .border_1()
                .border_color(cx.theme().border)
                .text_xs()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(cx.theme().foreground)
                .child(shell_integration::SCRIPT),
        )
}

/// 标题行：图标 + "命令历史" + 这台主机的名字。
fn render_header(host: &Host, cx: &App) -> impl IntoElement {
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
                .bg(palette::teal().opacity(0.12))
                .border_1()
                .border_color(palette::teal().opacity(0.35))
                .text_color(palette::teal())
                .child(Icon::new(AppIcon::History).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(t!("command_history.title"))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(host.name.clone()),
                ),
        )
}

impl Render for CommandHistoryDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.visible.len();
        let empty_note = self.empty_note();

        v_flex()
            .gap_2()
            // 顶上留出这 4px 不是为了好看：对话框正文是个会裁剪的滚动容器，
            // 而输入框聚焦时那圈 ring 画在自己的边框**外面**（gpui-kit 的
            // `focus_ring_style`，3px 环 + 1px 边框 = 向外 4px）。搜索框正好
            // 贴着正文顶边，不留这一道，环的上半圈就被裁掉 —— 左右下三边都有、
            // 独独上边是平的。
            .pt_1()
            .pb_1()
            .child(Input::new(&self.search).small().w_full())
            .child(
                div()
                    .h(LIST_HEIGHT)
                    .w_full()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    // 行是方角的，滚到边上不能盖住这一圈圆角。
                    .overflow_hidden()
                    .child(match rows {
                        0 => div()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(empty_note)
                            .into_any_element(),
                        _ => v_flex()
                            .id("command-history-list")
                            .relative()
                            .size_full()
                            .child(
                                v_virtual_list(
                                    cx.entity().clone(),
                                    "command-history-rows",
                                    self.row_sizes.clone(),
                                    |this, visible, _, cx| {
                                        visible.map(|row| this.render_row(row, cx)).collect()
                                    },
                                )
                                .track_scroll(&self.scroll),
                            )
                            .scrollbar(&self.scroll, ScrollbarAxis::Vertical)
                            .into_any_element(),
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!(
                        "command_history.hint",
                        limit => HISTORY_LIMIT,
                    ))),
            )
            .child(self.render_shell_integration(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{AuthMethod, Host, MonitorIntervals};
    use std::cell::RefCell;

    fn host() -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            kind: crate::host::HostKind::Ssh,
            id: "prod-k8s".into(),
            name: "Prod-K8s-Master-01".into(),
            user: "root".into(),
            hostname: "192.168.1.100".into(),
            port: 22,
            platform: "Ubuntu 24.04".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            shell_integration: false,
        }
    }

    /// 三条历史，把那一列的三种样子都占上：成了、败了、不知道。
    fn records() -> Vec<CommandRecord> {
        [
            ("docker compose up -d", Some(0)),
            ("cat /etc/nginx/missing.conf", Some(1)),
            ("tail -f /var/log/syslog", None),
        ]
        .into_iter()
        .enumerate()
        .map(|(at, (command, exit_code))| CommandRecord {
            id: format!("cmd-{at}").into(),
            command: command.into(),
            exit_code,
            executed_at: "2026-09-12 10:24:31".into(),
        })
        .collect()
    }

    struct Harness {
        dialog: Entity<CommandHistoryDialog>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            v_flex()
                .size_full()
                .child(render_header(&host(), cx))
                .child(self.dialog.clone())
        }
    }

    /// 把对话框挂进窗口画一帧。
    ///
    /// 外面套一层 [`Root`]：[`CommandHistoryDialog::run`] 要收起对话框层，
    /// 窗口第一层不是 `Root` 的话那一句直接 panic。里面再套一层 `Harness`，
    /// 是为了把标题那一段也画上 —— 它不在 [`Render`] 里（交给对话框外壳的），
    /// 单画正文覆盖不到，而它同样是一条会读实体状态的 builder 链。
    ///
    /// 快照是**直接塞进去**的，不走 `load` —— 那一步要跨到数据层的 tokio
    /// runtime 上，而 gpui 的测试调度器会把任何来自别的线程的唤醒判成不确定。
    /// 这里要验的是"这份数据画不画得出来"，读库归服务层那几个用例。
    fn draw(
        cx: &mut gpui_kit::TestAppContext,
        records: Vec<CommandRecord>,
        on_run: HistoryRunHandler,
    ) -> (
        Entity<CommandHistoryDialog>,
        &mut gpui_kit::VisualTestContext,
    ) {
        draw_with(cx, records, on_run, Rc::new(|_, _, _| {}))
    }

    /// 同上，但连 shell 集成那个开关的回调也接出来。
    fn draw_with(
        cx: &mut gpui_kit::TestAppContext,
        records: Vec<CommandRecord>,
        on_run: HistoryRunHandler,
        on_shell_integration: ShellIntegrationHandler,
    ) -> (
        Entity<CommandHistoryDialog>,
        &mut gpui_kit::VisualTestContext,
    ) {
        use gpui_kit::component::Root;

        // 开窗之前把这一下阻塞做完：之后一次异步都不能再发。
        let service = CommandHistoryService::in_memory_blocking();

        cx.update(gpui_kit::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let dialog = cx.new(|cx| {
                let mut this = CommandHistoryDialog::new(
                    host(),
                    service,
                    on_run,
                    on_shell_integration,
                    window,
                    cx,
                );
                this.records = records;
                this.loading = false;
                this.apply_filter(cx);
                this
            });
            Root::new(cx.new(|_| Harness { dialog }), window, cx)
        });
        let harness = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<Harness>()
                .expect("刚放进去的就是它")
        });
        let dialog = harness.read_with(cx, |harness, _| harness.dialog.clone());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (dialog, cx)
    }

    #[gpui_kit::test]
    fn the_dialog_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (_, cx) = draw(cx, records(), Rc::new(|_, _, _| {}));
        // 再画一帧：第一帧和后续帧走的不是同一条路（元素状态、焦点都只有
        // 第二帧才读得到），虚拟列表尤其如此 —— 可见区间是量过一帧才有的。
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 一条都没有时画的是另一条分支（空状态那句话），它同样要画得出来。
    #[gpui_kit::test]
    fn an_empty_history_draws_its_empty_state(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw(cx, Vec::new(), Rc::new(|_, _, _| {}));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        dialog.read_with(cx, |this, _| {
            // 读完库、确实一条没有 —— 这句话要点破"对端 shell 得打标记"，
            // 不然用户只会以为功能坏了。
            assert_eq!(
                this.empty_note(),
                SharedString::from(t!("command_history.empty"))
            );
        });
    }

    /// 还在读库时说的是"正在读取"，不是"没有历史"。两件事差得远。
    #[gpui_kit::test]
    fn a_dialog_that_is_still_loading_does_not_claim_to_be_empty(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (dialog, cx) = draw(cx, Vec::new(), Rc::new(|_, _, _| {}));
        cx.update(|_, cx| {
            dialog.update(cx, |this, _| this.loading = true);
        });
        dialog.read_with(cx, |this, _| {
            assert_eq!(
                this.empty_note(),
                SharedString::from(t!("command_history.loading"))
            );
        });
    }

    /// 搜索在快照上过滤，不回库里查。
    #[gpui_kit::test]
    fn the_filter_narrows_the_list(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw(cx, records(), Rc::new(|_, _, _| {}));

        dialog.update_in(cx, |this, window, cx| {
            this.search
                .update(cx, |input, cx| input.set_value("nginx", window, cx));
            // 程序改输入框的值不发 `Change`（那是敲键盘才有的事），
            // 所以这里照着订阅回调里那一句自己叫一次。
            this.apply_filter(cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        dialog.read_with(cx, |this, _| {
            assert_eq!(this.visible.len(), 1, "只有一条命令里带 nginx");
            assert_eq!(
                this.record_at(0).map(|record| record.command.as_ref()),
                Some("cat /etc/nginx/missing.conf")
            );
            // 行高表必须跟着可见行数走，不然虚拟列表算出来的可见区间是错的。
            assert_eq!(this.row_sizes.len(), this.visible.len());
        });
    }

    /// 点一行就是把那条命令发回会话。这是这一层存在的理由。
    #[gpui_kit::test]
    fn clicking_a_row_sends_that_command_back(cx: &mut gpui_kit::TestAppContext) {
        let sent: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let recorder = sent.clone();
        let (dialog, cx) = draw(
            cx,
            records(),
            Rc::new(move |command, _, _| *recorder.borrow_mut() = Some(command)),
        );

        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| this.run(0, window, cx));
        });

        assert_eq!(sent.borrow().as_deref(), Some("docker compose up -d"));
    }

    /// 扳到**开**不算数：先弹一层把那一行摆出来，用户没点头之前什么都不做。
    ///
    /// 这一条钉的是这个功能里最要紧的一句承诺 —— 我们要往用户的服务器上打字，
    /// 那一下必须是他亲手点的。开关自己也不许提前扳过去：它显示的是这台主机
    /// 现在的样子，不是我们打算把它变成的样子。
    #[gpui_kit::test]
    fn turning_it_on_asks_before_typing_anything(cx: &mut gpui_kit::TestAppContext) {
        let reported: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
        let recorder = reported.clone();
        let (dialog, cx) = draw_with(
            cx,
            records(),
            Rc::new(|_, _, _| {}),
            Rc::new(move |on, _, _| *recorder.borrow_mut() = Some(on)),
        );

        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.toggle_shell_integration(true, window, cx)
            });
        });
        // 确认框连同里头那块脚本都要画得出来 —— 用户看不到那一行就没法点头。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(
            cx.update(|window, cx| window.has_active_dialog(cx)),
            "扳到开应当先弹一层确认"
        );
        assert_eq!(*reported.borrow(), None, "还没点头就报上去了");
        dialog.read_with(cx, |this, _| {
            assert!(!this.shell_integration, "开关不该抢在确认之前扳过去");
        });
    }

    /// 点了"装上"才真的算数：开关跟着扳过去，新值也报给宿主。
    #[gpui_kit::test]
    fn confirming_flips_the_switch_and_reports_it(cx: &mut gpui_kit::TestAppContext) {
        let reported: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
        let recorder = reported.clone();
        let (dialog, cx) = draw_with(
            cx,
            records(),
            Rc::new(|_, _, _| {}),
            Rc::new(move |on, _, _| *recorder.borrow_mut() = Some(on)),
        );

        // 确认框那颗 OK 走的就是这一句。
        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.apply_shell_integration(true, window, cx)
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert_eq!(*reported.borrow(), Some(true));
        dialog.read_with(cx, |this, _| assert!(this.shell_integration));
    }

    /// 扳到**关**不弹框：停手不需要许可。
    #[gpui_kit::test]
    fn turning_it_off_takes_effect_at_once(cx: &mut gpui_kit::TestAppContext) {
        let reported: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
        let recorder = reported.clone();
        let (dialog, cx) = draw_with(
            cx,
            records(),
            Rc::new(|_, _, _| {}),
            Rc::new(move |on, _, _| *recorder.borrow_mut() = Some(on)),
        );
        cx.update(|_, cx| {
            dialog.update(cx, |this, _| this.shell_integration = true);
        });

        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.toggle_shell_integration(false, window, cx)
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert_eq!(*reported.borrow(), Some(false));
        assert!(
            !cx.update(|window, cx| window.has_active_dialog(cx)),
            "关掉不该弹任何东西"
        );
        dialog.read_with(cx, |this, _| assert!(!this.shell_integration));
    }

    /// 开关开着时画的是另一条分支（说明文字换了一句），它同样要画得出来。
    #[gpui_kit::test]
    fn the_switch_draws_in_both_positions(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw(cx, records(), Rc::new(|_, _, _| {}));
        cx.update(|_, cx| {
            dialog.update(cx, |this, cx| {
                this.shell_integration = true;
                cx.notify();
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    // 「清空」和行尾那颗「删除」都没有界面层的用例：
    // [`CommandHistoryDialog::clear`] 与 [`CommandHistoryDialog::delete`] 都会派
    // 一次落库到数据层的 tokio runtime 上，而 gpui 的测试调度器一看到来自别的
    // 线程的唤醒就判这个测试不确定（整个测试进程 abort，不只是这一条挂）。
    // 这两件事本身归服务层那两条用例
    // （`CommandHistoryService::tests::clearing_one_host_leaves_the_others_alone`、
    // `…::deleting_one_record_leaves_its_neighbours`）。
}
