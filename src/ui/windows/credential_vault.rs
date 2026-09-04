//! 保险箱窗口：把整个凭据保险箱铺到一扇独立的原生窗口里。
//!
//! 侧边栏那一栏只有 300 宽，三档筛选片得挤成两排、卡片只能一列往下排
//! （见 [`CredentialVaultView`]）。原型里这一屏本来是个盖住整个应用的大弹层
//! （`CredentialVaultModal`），做成弹层的代价是它和终端互斥 ——
//! 而"照着这条凭据登一台机器"恰恰要两屏同时在场。所以这里落成一扇窗：
//! 左栏筛选、右边一片卡片，能摊到第二块屏幕上，也能和主窗口并排。
//!
//! 版式照原型：
//!
//! | 段 | 内容 |
//! | --- | --- |
//! | 标题栏 | 系统窗口按钮那一条，只写窗口名 |
//! | 头部 | 盾牌图标、标题、存储说明徽章，右端两个生成器加"新建凭据" |
//! | 左栏 | 上面按档（连计数），下面按分组目录，底部是导出备份 |
//! | 右主区 | 搜索行（右端一句"共 N 条"），下面一片卡片 |
//!
//! 卡片铺几列跟着窗口宽度走（[`Self::columns`]）：这扇窗是能拉的，
//! 死钉两列的话拉宽之后就摊出两张一米宽的卡片。
//!
//! **保险箱本体不在这里**：那份快照、三道筛子、明文状态与所有会落库的动作都在
//! [`CredentialVaultView`] 上，这一屏持有它的一个实体，只另画一套版式。
//! 两个宿主各有各的一份快照，各自在窗口激活时重读。

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, IconName, Root, Sizable as _, StyledExt as _, TitleBar,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::Input,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Div, Entity, FocusHandle, Focusable, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, prelude::FluentBuilder as _, px, size,
};

use crate::{
    credential::{Credential, CredentialKind},
    service::CredentialService,
    ui::{
        cards::{CredentialAction, CredentialCard, kind_icon, kind_tone},
        theme::{AppIcon, palette},
        views::{CredentialVaultView, VaultFilter},
    },
};
use rust_i18n::t;

/// 左栏宽度。原型里是 224（`w-56`），这一栏放的是筛选与计数，宽度不随内容变。
const RAIL_WIDTH: Pixels = px(224.);

/// 各段共用的留白。比侧边栏那一档（8）宽一些 —— 这一屏不缺地方。
const GUTTER: Pixels = px(12.);

/// 相邻卡片之间的缝，横竖同一个数。
const CARD_GAP: Pixels = px(12.);

/// 一张凭据卡片舒服的宽度。铺几列按这个数分，见 [`CredentialVaultWindow::columns`]。
const CARD_TARGET_WIDTH: f32 = 460.;

/// 最多铺几列。再多的话一行里的卡片就开始比它们之间的空隙更难扫了。
const MAX_COLUMNS: usize = 4;

/// 行内图标的尺寸。
const ICON_SIZE: Pixels = px(15.);

/// 头部那个盾牌图标底下的方块边长。
const BADGE_SIZE: Pixels = px(34.);

/// 左栏上面那一段的一行：这一行按哪一档筛、叫什么、什么图标、有几条。
struct FilterRow {
    filter: VaultFilter,
    label: SharedString,
    icon: Icon,
    count: usize,
}

/// 已经开着的那扇保险箱窗口。
///
/// 单例是这一屏自己的事，不摊给调用方：活动栏、命令面板、侧边栏三个入口
/// 点的都是同一个 [`CredentialVaultWindow::open`]，谁都不必先问"开着没有"。
/// 窗口被关掉之后这个句柄还留在全局里，但它已经失效了 ——
/// 下一次 `open` 拿它去激活会失败，那时就重新开一扇。
struct OpenVaultWindow(WindowHandle<Root>);

impl Global for OpenVaultWindow {}

pub struct CredentialVaultWindow {
    /// 保险箱本体。这一屏所有的读与写都经过它。
    vault: Entity<CredentialVaultView>,
    focus_handle: FocusHandle,
    /// 系统深浅设置的订阅。每扇窗口各订一份 —— 丢掉就等于取消，
    /// 所以它得挂在这一屏身上活着。
    _appearance: Subscription,
}

impl CredentialVaultWindow {
    /// 打开保险箱窗口；已经开着就把它叫到前面来。
    pub fn open(service: CredentialService, cx: &mut App) {
        if let Some(open) = cx.try_global::<OpenVaultWindow>() {
            let handle = open.0;
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
        }

        // 开在屏幕正中，一个够用的尺寸：左栏、搜索行和两列卡片都摆得下。
        // 不最大化也不全屏 —— 这扇窗的用法是和主窗口来回切，
        // 铺满屏幕该是用户自己按下的那一下（列数会跟着变，见 `columns`）。
        let mut window_size = size(px(1280.), px(860.));
        if let Some(display) = cx.primary_display() {
            let display_size = display.bounds().size;
            window_size.width = window_size.width.min(display_size.width * 0.9);
            window_size.height = window_size.height.min(display_size.height * 0.9);
        }

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                window_size,
                cx,
            ))),
            // 低于这个尺寸，左栏加一列卡片就摆不下了。
            window_min_size: Some(size(px(720.), px(520.))),
            kind: WindowKind::Normal,
            #[cfg(target_os = "linux")]
            window_background: gpui_kit::WindowBackgroundAppearance::Transparent,
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui_kit::WindowDecorations::Client),
            ..TitleBar::window_options()
        };

        let opened = cx.open_window(options, |window, cx| {
            let shell = cx.new(|cx| Self::new(service, window, cx));
            // 每个窗口的第一层必须是 Root，它承载对话框、通知与侧板 ——
            // 这一屏的表单、确认框和通知因此都落在它自己身上，不飘回主窗口。
            cx.new(|cx| Root::new(shell, window, cx))
        });

        let handle = match opened {
            Ok(handle) => handle,
            Err(error) => {
                log::error!("打开保险箱窗口失败：{error:#}");
                return;
            }
        };

        _ = handle.update(cx, |_, window, cx| {
            window.activate_window();
            window.set_window_title(&t!("vault_window.window_title"));
            // 系统深浅设置在 Linux 上是开窗之后才送达的，订阅之后补上。
            crate::ui::observe_appearance(window, cx).detach();
        });
        cx.set_global(OpenVaultWindow(handle));
    }

    fn new(service: CredentialService, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let vault = cx.new(|cx| CredentialVaultView::new(service, window, cx));
        // 保险箱一动这一屏就得重画：卡片、计数、筛选状态全从它那儿来。
        cx.observe(&vault, |_, _, cx| cx.notify()).detach();

        // 开窗第一件事多半是找一条凭据，焦点就落在搜索框上。`defer` 是必须的：
        // 这一刻整棵树还没画过，焦点句柄还没进这一帧的焦点树。
        let search = vault.read(cx).search_state().clone();
        window.defer(cx, move |window, cx| {
            search.update(cx, |state, cx| state.focus(window, cx));
        });

        Self {
            vault,
            focus_handle: cx.focus_handle(),
            _appearance: crate::ui::observe_appearance(window, cx),
        }
    }

    /// 这一屏铺几列卡片。
    ///
    /// 按可用宽度分，不钉死列数：这扇窗拉到 4K 屏那么宽的时候，
    /// 两列摊出来的卡片里那行指纹和右边的按钮会隔着半米；
    /// 拉窄到只剩一列时也照样铺得下。
    fn columns(&self, window: &Window) -> usize {
        let content = f32::from(window.viewport_size().width - RAIL_WIDTH - GUTTER * 2.);
        // 负数（窗口比左栏还窄）转成 usize 会饱和到 0，随后被 clamp 抬回 1。
        ((content / CARD_TARGET_WIDTH).floor() as usize).clamp(1, MAX_COLUMNS)
    }

    /// 标题栏：只写窗口名。设计稿那一整块标题在它下面那一段里 ——
    /// 34 高的标题栏放不下"图标 + 标题 + 徽章 + 副标题"，
    /// 而这一条还得留给系统的窗口按钮。
    fn render_title_bar(&self) -> impl IntoElement {
        TitleBar::new().child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(AppIcon::ShieldCheck)
                        .with_size(px(14.))
                        .flex_shrink_0()
                        .text_color(palette::teal()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(SharedString::from(t!("vault_window.window_title"))),
                ),
        )
    }

    /// 头部：盾牌、标题与那句存储说明，右端两个生成器加"新建凭据"。
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .w_full()
            .px(GUTTER)
            .py(px(10.))
            .gap_3()
            .items_center()
            .bg(palette::recessed(cx))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_shrink_0()
                    .size(BADGE_SIZE)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(cx.theme().radius)
                    .bg(palette::teal().opacity(0.1))
                    .border_1()
                    .border_color(palette::teal().opacity(0.3))
                    .text_color(palette::teal())
                    .child(Icon::new(AppIcon::ShieldCheck).with_size(px(18.))),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_sm()
                                    .font_semibold()
                                    .child(SharedString::from(t!("vault_window.title"))),
                            )
                            .child(self.render_storage_badge()),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(t!("vault_window.subtitle"))),
                    ),
            )
            .child(self.render_header_actions(cx))
    }

    /// 标题旁边那颗徽章。
    ///
    /// 原型上印的是"AES-256 加密保护"，这里印的是实话：库文件里存的是明文
    /// （见 `vault.storage_note`）。一颗说谎的安全徽章比没有徽章坏得多 ——
    /// 用户会照着它决定要不要把生产 root 密码放进来。
    fn render_storage_badge(&self) -> impl IntoElement {
        let tone = palette::amber();
        div()
            .flex_shrink_0()
            .px_1p5()
            .rounded_full()
            .bg(tone.opacity(0.12))
            .border_1()
            .border_color(tone.opacity(0.35))
            .font_family("monospace")
            .text_xs()
            .text_color(tone)
            .child(SharedString::from(t!("vault_window.badge")))
    }

    /// 右端那三个按钮：两个生成器（安静一档）加主操作"新建凭据"。
    fn render_header_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let secondary = ButtonCustomVariant::new(cx)
            .color(palette::surface(cx))
            .foreground(cx.theme().foreground)
            .hover(palette::surface_strong(cx))
            .active(palette::surface_strong(cx));
        let blue = palette::blue();
        let primary = ButtonCustomVariant::new(cx)
            .color(blue)
            .foreground(palette::on_accent())
            .hover(blue.lighten(0.08))
            .active(blue.darken(0.08));

        h_flex()
            .flex_shrink_0()
            .gap_2()
            .items_center()
            .child(
                Button::new("credential-vault-window-password-generator")
                    .custom(secondary)
                    .small()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .icon(Icon::new(AppIcon::Sparkles).text_color(palette::amber()))
                    .label(t!("vault.generate_password"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.vault
                            .update(cx, |vault, cx| vault.open_password_generator(window, cx));
                    })),
            )
            .child(
                Button::new("credential-vault-window-key-generator")
                    .custom(secondary)
                    .small()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .icon(Icon::new(AppIcon::FileCode).text_color(palette::teal()))
                    .label(t!("vault.generate_keypair"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.vault
                            .update(cx, |vault, cx| vault.open_key_generator(window, cx));
                    })),
            )
            .child(
                Button::new("credential-vault-window-new")
                    .custom(primary)
                    // Custom 变体的常态底色会被稀释两成，这里把设计稿的实色压回去。
                    .bg(blue)
                    .small()
                    .rounded(cx.theme().radius)
                    .icon(IconName::Plus)
                    .label(t!("vault.new_credential"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.vault
                            .update(cx, |vault, cx| vault.new_credential(window, cx));
                    })),
            )
    }

    /// 左栏：上面按档，下面按分组目录，底部导出备份。
    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 先把要用的东西从保险箱里取出来：下面搭按钮要 `cx.listener`（可变借用），
        // 而 `read` 借的是同一个 `cx`。
        let filter = self.vault.read(cx).filter();
        let category = self.vault.read(cx).category();
        let categories = self.vault.read(cx).categories();
        let kinds = self.kind_rows(cx);

        let rows: Vec<_> = kinds
            .into_iter()
            .enumerate()
            .map(|(ix, row)| {
                let selected = filter == row.filter;
                self.render_filter_row(ix, row, selected, cx)
            })
            .collect();

        // 第一片说的是"不按目录筛"，不是那个叫"未分组"的目录。
        let mut category_rows = vec![self.render_category_row(
            0,
            None,
            t!("vault_window.all_categories").into(),
            category.is_none(),
            cx,
        )];
        for (ix, name) in categories.iter().enumerate() {
            let selected = category.as_ref() == Some(name);
            category_rows.push(self.render_category_row(
                ix + 1,
                Some(name.clone()),
                name.clone(),
                selected,
                cx,
            ));
        }

        v_flex()
            .flex_shrink_0()
            .w(RAIL_WIDTH)
            .h_full()
            .justify_between()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .id("credential-vault-window-rail")
                    .flex_1()
                    .min_h_0()
                    .p(px(8.))
                    .gap_4()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(section_label(t!("vault_window.section_kinds").into(), cx))
                            .children(rows),
                    )
                    // 一条都没归类时这一段整个不画：那时它只有一个"全部目录"，
                    // 点了和不点一样。
                    .when(!categories.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .gap_0p5()
                                .pt_2()
                                .border_t_1()
                                .border_color(cx.theme().sidebar_border)
                                .child(section_label(t!("vault_window.section_groups").into(), cx))
                                .children(category_rows),
                        )
                    }),
            )
            .child(self.render_rail_footer(cx))
    }

    /// 上面那一段的五行：全部、三档、特别关注。每行右端是它的条数。
    fn kind_rows(&self, cx: &App) -> Vec<FilterRow> {
        let credentials = self.vault.read(cx).credentials();
        let mut rows = vec![FilterRow {
            filter: VaultFilter::All,
            label: SharedString::from(t!("vault_window.all")),
            icon: Icon::new(IconName::FolderClosed).text_color(palette::teal()),
            count: credentials.len(),
        }];
        rows.extend(CredentialKind::ALL.into_iter().map(|kind| {
            FilterRow {
                filter: VaultFilter::Kind(kind),
                label: kind.label(),
                // 分档色和卡片上那个小方块是同一份：筛到"SSH 私钥"时，
                // 左栏亮着的那一行和右边一片卡片的图标底色对得上。
                icon: Icon::new(kind_icon(kind)).text_color(kind_tone(kind)),
                count: credentials
                    .iter()
                    .filter(|credential| credential.kind == kind)
                    .count(),
            }
        }));
        rows.push(FilterRow {
            filter: VaultFilter::Favorite,
            label: SharedString::from(t!("vault.filter_favorite")),
            icon: Icon::new(IconName::StarFill).text_color(palette::amber()),
            count: credentials
                .iter()
                .filter(|credential| credential.favorite)
                .count(),
        });
        rows
    }

    /// 返回具体的 [`Button`] 而不是 `impl IntoElement`：后者会隐式捕获 `cx` 的
    /// 生命周期，装进 `Vec` 之后借用就跟着跑出闭包了。
    fn render_filter_row(
        &self,
        ix: usize,
        row: FilterRow,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        let FilterRow {
            filter,
            label,
            icon,
            count,
        } = row;
        let variant = ButtonCustomVariant::new(cx)
            .color(cx.theme().transparent)
            .foreground(match selected {
                true => palette::teal(),
                false => cx.theme().sidebar_foreground,
            })
            .hover(palette::surface(cx))
            .active(palette::surface_strong(cx));

        Button::new(("credential-vault-window-filter", ix))
            .custom(variant)
            .w_full()
            .rounded(cx.theme().radius)
            .when(selected, |this| {
                this.bg(palette::blue().opacity(0.15))
                    .border_1()
                    .border_color(palette::blue().opacity(0.4))
            })
            // 这一行的内容是"图标 + 名字 + 计数"三格，没有 `label`，
            // 读屏软件念不出它是什么，名字在这里补上。
            .accessibility_label(label.clone())
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(icon.with_size(ICON_SIZE).flex_shrink_0())
                            .child(div().flex_1().min_w_0().truncate().text_xs().child(label)),
                    )
                    .child(count_badge(count, cx)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.vault
                    .update(cx, |vault, cx| vault.set_filter(filter, cx));
            }))
    }

    /// 下面那一段的一行。比上面安静一档：它是第二道筛子，选中只把字点亮。
    fn render_category_row(
        &self,
        ix: usize,
        category: Option<SharedString>,
        label: SharedString,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        let variant = ButtonCustomVariant::new(cx)
            .color(match selected {
                true => palette::surface(cx),
                false => cx.theme().transparent,
            })
            .foreground(match selected {
                true => palette::teal(),
                false => palette::idle(cx),
            })
            .hover(palette::surface(cx))
            .active(palette::surface_strong(cx));

        Button::new(("credential-vault-window-category", ix))
            .custom(variant)
            .xsmall()
            .w_full()
            .rounded(cx.theme().radius)
            .accessibility_label(label.clone())
            .child(div().w_full().truncate().text_left().text_xs().child(label))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.vault
                    .update(cx, |vault, cx| vault.set_category(category.clone(), cx));
            }))
    }

    /// 左栏底部：导出备份，下面一句实话 —— 这些东西存在哪儿、加没加密。
    fn render_rail_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let variant = ButtonCustomVariant::new(cx)
            .color(palette::surface(cx))
            .foreground(cx.theme().sidebar_foreground)
            .hover(palette::surface_strong(cx))
            .active(palette::surface_strong(cx));

        v_flex()
            .flex_shrink_0()
            .p(px(8.))
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                Button::new("credential-vault-window-export")
                    .custom(variant)
                    .xsmall()
                    .w_full()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().sidebar_border)
                    .icon(Icon::new(AppIcon::Download).text_color(palette::teal()))
                    .label(t!("vault.export_backup"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.vault
                            .update(cx, |vault, cx| vault.export_backup(window, cx));
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_center()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("vault.storage_note"))),
            )
    }

    /// 右主区：搜索行加一片卡片。
    fn render_main(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .child(self.render_search_bar(cx))
            .child(self.render_list(window, cx))
    }

    /// 搜索行：左边搜索框，右端一句"共 N 条凭据"。
    ///
    /// 那个 N 是**筛完之后**的条数，不是保险箱里的总数：它回答的是
    /// "我这一屏正在看多少条"。总数在左栏"全部凭据"那一行上。
    fn render_search_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let search = self.vault.read(cx).search_state().clone();
        let shown = self.vault.read(cx).visible(cx).len();

        h_flex()
            .flex_shrink_0()
            .w_full()
            .px(GUTTER)
            .py_2()
            .gap_3()
            .items_center()
            .justify_between()
            .bg(palette::recessed(cx))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div().flex_1().min_w_0().max_w(px(440.)).child(
                    Input::new(&search)
                        .cleanable(true)
                        .prefix(
                            Icon::new(IconName::Search)
                                .with_size(ICON_SIZE)
                                .text_color(palette::idle(cx)),
                        )
                        .w_full()
                        .bg(palette::surface(cx)),
                ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("vault_window.count", count => shown))),
            )
    }

    /// 卡片区。一行 [`Self::columns`] 张，最后一行不满就补空位。
    fn render_list(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.vault.read(cx).visible(cx);
        let vault_is_empty = self.vault.read(cx).credentials().is_empty();
        let columns = self.columns(window);

        // 先渲染出来再交给 `children`：闭包借了 `cx`，惰性迭代器会把这个借用
        // 带出闭包体，borrow checker 不放行。
        let rows: Vec<_> = visible
            .chunks(columns)
            .enumerate()
            .map(|(row, chunk)| self.render_row(row, chunk, columns, cx))
            .collect();

        div()
            .flex_1()
            .min_h_0()
            .bg(cx.theme().background)
            .map(|this| match rows.is_empty() {
                true => this.child(self.render_empty(vault_is_empty, cx)),
                false => this.child(
                    v_flex()
                        .id("credential-vault-window-list")
                        .size_full()
                        .p(GUTTER)
                        .gap(CARD_GAP)
                        .overflow_y_scroll()
                        .children(rows),
                ),
            })
    }

    /// 一行卡片。
    ///
    /// 不做虚拟滚动，理由和侧边栏那一列一样：卡片的高度随标签行数与分档变化，
    /// 量不出一个统一的行高，而保险箱是攒出来的几十条。
    fn render_row(
        &self,
        row: usize,
        chunk: &[Credential],
        columns: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut line = h_flex().w_full().items_start().gap(CARD_GAP);
        for (column, credential) in chunk.iter().enumerate() {
            line = line.child(div().flex_1().min_w_0().child(self.render_card(
                row * columns + column,
                credential.clone(),
                cx,
            )));
        }
        // 最后一行不满时补几个空位，剩下那一两张卡片才不会被拉成整行宽 ——
        // 一屏卡片里突然有一张两倍宽的，读起来像是它特别重要。
        for _ in chunk.len()..columns {
            line = line.child(div().flex_1().min_w_0());
        }
        line
    }

    /// 返回具体的 [`CredentialCard`] 而不是 `impl IntoElement`：同上，
    /// 后者会把 `cx` 的借用带出闭包体。
    fn render_card(
        &self,
        ix: usize,
        credential: Credential,
        cx: &mut Context<Self>,
    ) -> CredentialCard {
        let revealed = self.vault.read(cx).is_revealed(&credential.id);
        let actioned = credential.clone();

        CredentialCard::new(("credential-vault-window-card", ix), credential)
            .revealed(revealed)
            .on_action(
                cx.listener(move |this, action: &CredentialAction, window, cx| {
                    let action = *action;
                    let credential = actioned.clone();
                    this.vault.update(cx, |vault, cx| {
                        vault.run_action(action, &credential, window, cx)
                    });
                }),
            )
    }

    /// 空状态。整个保险箱空着的时候多说一句它能装什么，并把"新建"摆在手边 ——
    /// 这一屏是保险箱的正门，第一次打开时它就是唯一的说明书。
    fn render_empty(&self, vault_is_empty: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let blue = palette::blue();
        let variant = ButtonCustomVariant::new(cx)
            .color(blue)
            .foreground(palette::on_accent())
            .hover(blue.lighten(0.08))
            .active(blue.darken(0.08));

        v_flex()
            .size_full()
            .p_6()
            .gap_3()
            .items_center()
            .justify_center()
            .text_center()
            .child(
                div()
                    .size(px(48.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(palette::surface(cx))
                    .border_1()
                    .border_color(cx.theme().border)
                    .text_color(palette::idle(cx))
                    .child(Icon::new(AppIcon::ShieldCheck).with_size(px(22.))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .items_center()
                    .child(div().text_sm().font_semibold().child(SharedString::from(
                        match vault_is_empty {
                            true => t!("vault.empty"),
                            false => t!("vault.no_match"),
                        },
                    )))
                    .child(div().text_xs().text_color(palette::idle(cx)).child(
                        SharedString::from(match vault_is_empty {
                            true => t!("vault.empty_hint"),
                            false => t!("vault_window.no_match_hint"),
                        }),
                    )),
            )
            .child(
                Button::new("credential-vault-window-empty-new")
                    .custom(variant)
                    .bg(blue)
                    .small()
                    .rounded(cx.theme().radius)
                    .icon(IconName::Plus)
                    .label(t!("vault_window.new_now"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.vault
                            .update(cx, |vault, cx| vault.new_credential(window, cx));
                    })),
            )
    }
}

/// 左栏里一段的小标题。
fn section_label(label: SharedString, cx: &App) -> impl IntoElement {
    div()
        .px_1p5()
        .py_0p5()
        .text_xs()
        .font_semibold()
        .text_color(palette::idle(cx))
        .child(label)
}

/// 左栏那一行右端的条数。等宽字体、凹陷底色：它是嵌在那一行里的一格读数。
fn count_badge(count: usize, cx: &App) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .px_1()
        .rounded_sm()
        .bg(palette::surface_strong(cx))
        .font_family("monospace")
        .text_xs()
        .text_color(palette::idle(cx))
        .child(SharedString::from(count.to_string()))
}

impl Focusable for CredentialVaultWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CredentialVaultWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sheet_layer = Root::render_sheet_layer(window, cx);
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        div()
            .id("credential-vault-window")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .v_flex()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar())
            .child(self.render_header(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.render_rail(cx))
                    .child(self.render_main(window, cx)),
            )
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::KeyAlgorithm;

    fn credential(name: &str, kind: CredentialKind, category: &str) -> Credential {
        Credential {
            id: SharedString::from(format!("cred-{name}")),
            name: name.into(),
            kind,
            category: category.into(),
            username: "root".into(),
            password: "s3cr3t".into(),
            algorithm: KeyAlgorithm::Ed25519,
            public_key: "ssh-ed25519 AAAA host".into(),
            private_key: "-----BEGIN OPENSSH PRIVATE KEY-----".into(),
            passphrase: "".into(),
            fingerprint: "SHA256:abc".into(),
            token: "eyJhbGciOi".into(),
            tags: vec!["SSH".into()],
            notes: "".into(),
            favorite: false,
            updated_at: "2025-01-10 09:30:00".into(),
        }
    }

    /// 搭一屏保险箱窗口的**内容**，不真的开一扇窗。
    ///
    /// 走 [`CredentialVaultView::from_snapshot`] 而不是
    /// [`CredentialVaultWindow::open`]：那条路会开一扇真窗口、连真库、
    /// 再派一次异步加载，三样在测试里都不能有（见那个构造器的文档）。
    /// 测的是这一屏画得通不通 —— 深 builder 链上的错 `cargo check` 看不见。
    fn draw_window(
        cx: &mut gpui_kit::TestAppContext,
        credentials: Vec<Credential>,
    ) -> &mut gpui_kit::VisualTestContext {
        cx.update(gpui_kit::init);
        let service = CredentialService::in_memory_blocking();
        let (_shell, cx) = cx.add_window_view(|window, cx| {
            let vault =
                cx.new(|cx| CredentialVaultView::from_snapshot(service, credentials, window, cx));
            CredentialVaultWindow {
                vault,
                focus_handle: cx.focus_handle(),
                _appearance: crate::ui::observe_appearance(window, cx),
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx
    }

    /// 整屏画得通：标题栏、头部、左栏两段、搜索行和一片卡片。
    ///
    /// 三档凭据各来一条，而且分了组 —— 左栏那两段和三种卡片正文都得走到。
    /// 一屏上同时画好几张卡片还能查出撞车的元素 id：debug build 下 a11y 树
    /// 建到第二个就直接断言崩。
    #[gpui_kit::test]
    fn the_window_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let cx = draw_window(
            cx,
            vec![
                credential("prod-root", CredentialKind::Password, "Linux"),
                credential("aws-key", CredentialKind::PrivateKey, "云服务"),
                credential("k8s-token", CredentialKind::Token, "Linux"),
            ],
        );
        // 画两帧：第一帧和后续帧走的不是同一条路。
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 空保险箱画的是空状态那一段（连"立即新建凭据"），而不是一片空白。
    #[gpui_kit::test]
    fn an_empty_vault_draws_its_empty_state(cx: &mut gpui_kit::TestAppContext) {
        let cx = draw_window(cx, Vec::new());
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 一条都没归类时左栏那第二段整个不画 —— 那时它只有一个"全部目录"。
    #[gpui_kit::test]
    fn the_group_section_disappears_without_groups(cx: &mut gpui_kit::TestAppContext) {
        let cx = draw_window(cx, vec![credential("野生的", CredentialKind::Password, "")]);
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
}
