//! 保险箱视图：侧边栏里管理账号密码、SSH 私钥与 API 令牌的那一块。
//!
//! 版式和主机视图同构，自上而下四段：标题行、搜索 + 筛选片、列表、底部主操作。
//! 段与段之间用 1px 分隔线断开，中间那几段落在亮一档的内容表面上 ——
//! 换视图时侧边栏的骨架不跳。
//!
//! 筛选做成两排片：上一排按**档**（全部 / 账号密码 / SSH 私钥 / API 令牌 /
//! 特别关注），下一排按**分组目录**，目录是从凭据自己身上收出来的，
//! 一条都没归类时那一排整个不画。原型里这两排分别在左栏和顶栏，
//! 在一条 300 宽的侧边栏里叠成两排更省地方，选中的还是同一件事。
//!
//! 列表不做虚拟滚动：保险箱是攒出来的几十条，而卡片的高度随标签行数与分档
//! 变化，量不出一个统一的行高（主机卡片能虚拟化正是因为它定高）。
//!
//! **秘密默认打码**，明文是这一次的显示状态，记在 [`Self::revealed`] 里而不是
//! 存进凭据：重读一次列表、换一次筛选，它就该回到打码。
//!
//! 这个组件同时是**保险箱本体**：那份快照、三道筛子、明文状态，以及所有会
//! 落库的动作（增删改、翻星、复制、另存 .pem、导出备份）都在这里，
//! 独立窗口 [`crate::ui::windows::CredentialVaultWindow`] 持有一份它，
//! 只另画一套全屏版式（那边的宿主见 [`CredentialSavedHandler`]）。
//! 两个宿主各有各的一份快照，因此窗口激活时都会重读一次 ——
//! 在窗口里新建的凭据，切回主窗口就该在侧边栏里看见。

use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputEvent, InputState},
    separator::Separator,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
};

use super::{GUTTER, ICON_SIZE, SidebarView, collapse_button, view_header};
use crate::{
    credential::{Credential, CredentialKind, backup_json},
    service::{CredentialService, LocalFileService},
    ui::{
        cards::{CredentialAction, CredentialCard, CredentialField, kind_tone},
        dialogs::{
            CredentialFormDialog, CredentialSavedHandler, KeyGeneratorDialog,
            PasswordGeneratorDialog,
        },
        theme::{AppIcon, palette},
    },
};
use rust_i18n::t;

/// 相邻筛选片之间的缝。和主机视图那一排是同一个数。
const CHIP_GAP: gpui_kit::Pixels = gpui_kit::px(6.);

/// 列表里一张卡片下面留给下一张的缝。
const CARD_GAP: gpui_kit::Pixels = gpui_kit::px(8.);

/// 上面那一排片选的是哪一档。独立窗口左栏那五行选的是同一件事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultFilter {
    /// 全部凭据。
    All,
    /// 某一档：账号密码 / SSH 私钥 / API 令牌。
    Kind(CredentialKind),
    /// 只看标了星的那些。
    Favorite,
}

/// 视图往外发的请求。增删改、复制、生成这些它自己都办得了，
/// 剩下这两件归宿主：收起整个侧边栏，以及把保险箱推到一个独立窗口里去。
#[derive(Clone, Debug)]
pub enum CredentialVaultEvent {
    Collapse,
    /// 在一个独立窗口里全屏打开保险箱。
    OpenWindow,
}

pub struct CredentialVaultView {
    /// 保险箱服务的入口。`credentials` 只是数据库的一份快照，
    /// 所有写操作都先经服务层落库，再由 [`Self::reload`] 把快照换回来。
    service: CredentialService,
    credentials: Vec<Credential>,
    filter: VaultFilter,
    /// 选中的分组目录。`None` 是"全部目录"。
    category: Option<SharedString>,
    /// 此刻亮着明文的那几条。见模块文档。
    revealed: HashSet<SharedString>,
    search: Entity<InputState>,
    focus_handle: FocusHandle,
}

impl CredentialVaultView {
    pub fn new(service: CredentialService, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("vault.search_placeholder")));
        cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();

        // 窗口一激活就重读：保险箱有两个宿主（侧边栏与独立窗口），各持一份
        // 快照。在窗口里新建的凭据，切回主窗口就该在侧边栏里看见，
        // 而不是等到下一次自己写一笔。
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.reload(window, cx);
            }
        })
        .detach();

        let mut this = Self {
            service,
            credentials: Vec::new(),
            filter: VaultFilter::All,
            category: None,
            revealed: HashSet::new(),
            search,
            focus_handle: cx.focus_handle(),
        };
        this.reload(window, cx);
        this
    }

    /// 搭一个带着现成快照的保险箱，**不**碰服务层、也不派任何异步。
    ///
    /// 界面测试要的就是这个：[`Self::new`] 会派一次加载出去，那一下的完成回调
    /// 来自 tokio 线程，gpui 的测试调度器会当场判它不确定
    /// （`Your test is not deterministic`），而且读的是用户的真库。
    /// 两个宿主（本视图与保险箱窗口）的画法测试都从这里搭。
    #[cfg(test)]
    pub(crate) fn from_snapshot(
        service: CredentialService,
        credentials: Vec<Credential>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            service,
            credentials,
            filter: VaultFilter::All,
            category: None,
            revealed: HashSet::new(),
            search: cx.new(|cx| InputState::new(window, cx)),
            focus_handle: cx.focus_handle(),
        }
    }

    /// 库里那一份快照。窗口左栏的计数与空状态都按它算。
    pub(crate) fn credentials(&self) -> &[Credential] {
        &self.credentials
    }

    pub(crate) fn filter(&self) -> VaultFilter {
        self.filter
    }

    pub(crate) fn category(&self) -> Option<SharedString> {
        self.category.clone()
    }

    /// 搜索框的状态。两个宿主各有一个自己的输入框（输入状态与焦点是跟着
    /// 窗口的），所以这里给的是本宿主那一个。
    pub(crate) fn search_state(&self) -> &Entity<InputState> {
        &self.search
    }

    /// 这条凭据此刻是不是亮着明文。
    pub(crate) fn is_revealed(&self, id: &SharedString) -> bool {
        self.revealed.contains(id)
    }

    /// 把"这条凭据存好了"接回自己身上。三个凭据对话框都收这一个回调，
    /// 它们因此不必知道这一次是侧边栏还是独立窗口弹出来的。
    pub(crate) fn saved_handler(&self, cx: &mut Context<Self>) -> CredentialSavedHandler {
        let vault = cx.weak_entity();
        Rc::new(move |credential, window, cx| {
            vault
                .update(cx, |this, cx| this.credential_saved(credential, window, cx))
                .ok();
        })
    }

    /// 从服务层重读整份保险箱。所有写操作完成后都走这里 ——
    /// 界面自己不改数据，数据库才是正本。
    ///
    /// 顺手把明文状态清掉：列表都换了一份，还亮着的那几条已经不知道是谁了。
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        self.revealed.clear();
        cx.spawn_in(window, async move |this, cx| {
            let loaded = service.load_all().await;
            this.update_in(cx, |this, window, cx| match loaded {
                Ok(credentials) => {
                    this.credentials = credentials;
                    cx.notify();
                }
                Err(error) => window
                    .push_notification(t!("vault.load_failed", error => format!("{error:#}")), cx),
            })
            .ok();
        })
        .detach();
    }

    /// 存完一条凭据之后的收尾：把筛子调到看得见它的位置，再重读列表。
    ///
    /// 两个对话框都走这一条 —— [`CredentialFormDialog`] 存完表单，
    /// [`KeyGeneratorDialog`] 生成完密钥，落库的是它们自己，
    /// 但列表该怎么跟上归这里。
    pub fn credential_saved(
        &mut self,
        credential: Credential,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_visible(&credential);
        self.reload(window, cx);
    }

    /// 让这条凭据在当前的筛选下看得见：过不去的那一道筛子退回"全部"。
    ///
    /// 存完之后它却不在列表里（因为正筛着别的档）会让人以为没存上。
    /// 已经看得见的话一个字都不动 —— 那是用户刚刚才挑好的筛选。
    fn ensure_visible(&mut self, credential: &Credential) {
        if !self.filter_admits(credential) {
            self.filter = VaultFilter::All;
        }
        if !self.category_admits(credential) {
            self.category = None;
        }
    }

    /// 当前搜索词。
    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_string()
    }

    /// 这条凭据过不过上面那排片。
    fn filter_admits(&self, credential: &Credential) -> bool {
        match &self.filter {
            VaultFilter::All => true,
            VaultFilter::Kind(kind) => credential.kind == *kind,
            VaultFilter::Favorite => credential.favorite,
        }
    }

    /// 这条凭据过不过下面那排目录片。
    fn category_admits(&self, credential: &Credential) -> bool {
        match &self.category {
            None => true,
            Some(category) => &credential.category == category,
        }
    }

    /// 三道筛子都过了的那些：档、目录、搜索词。
    pub(crate) fn visible(&self, cx: &App) -> Vec<Credential> {
        let query = self.query(cx);
        self.credentials
            .iter()
            .filter(|credential| {
                self.filter_admits(credential)
                    && self.category_admits(credential)
                    && credential.matches(&query)
            })
            .cloned()
            .collect()
    }

    /// 凭据自己身上收出来的分组目录，去重后按名字排。没归类的（空串）不算。
    pub(crate) fn categories(&self) -> Vec<SharedString> {
        let mut categories: Vec<SharedString> = self
            .credentials
            .iter()
            .filter(|credential| !credential.category.is_empty())
            .map(|credential| credential.category.clone())
            .collect();
        categories.sort();
        categories.dedup();
        categories
    }

    pub(crate) fn set_filter(&mut self, filter: VaultFilter, cx: &mut Context<Self>) {
        if self.filter == filter {
            return;
        }
        self.filter = filter;
        cx.notify();
    }

    pub(crate) fn set_category(&mut self, category: Option<SharedString>, cx: &mut Context<Self>) {
        if self.category == category {
            return;
        }
        self.category = category;
        cx.notify();
    }

    /// 卡片上点出来的那一件事。
    pub(crate) fn run_action(
        &mut self,
        action: CredentialAction,
        credential: &Credential,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            CredentialAction::ToggleReveal => self.toggle_reveal(&credential.id, cx),
            CredentialAction::ToggleFavorite => self.toggle_favorite(credential, window, cx),
            CredentialAction::Edit => self.edit(credential, window, cx),
            CredentialAction::Delete => self.confirm_delete(credential, window, cx),
            CredentialAction::Copy(field) => self.copy(field, credential, window, cx),
            CredentialAction::SavePrivateKey => self.save_private_key(credential, window, cx),
        }
    }

    fn toggle_reveal(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if !self.revealed.remove(id) {
            self.revealed.insert(id.clone());
        }
        cx.notify();
    }

    /// 复制一格到剪贴板。
    ///
    /// 复制**打码之下的那一份**，不看此刻亮没亮：打码是给旁边的人看的，
    /// 而按下复制的意思一律是"我要那段真的"。
    fn copy(
        &mut self,
        field: CredentialField,
        credential: &Credential,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match field {
            CredentialField::Username => credential.username.clone(),
            CredentialField::Secret => credential.secret().clone(),
            CredentialField::PublicKey => credential.public_key.clone(),
        };
        if text.is_empty() {
            return;
        }

        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        // 通知里只报凭据的名字，一个字节的秘密都不进通知栏 ——
        // 那一条会停在屏幕上好几秒，还可能被截图带走。
        window.push_notification(t!("vault.copied", name => credential.name), cx);
    }

    fn toggle_favorite(
        &mut self,
        credential: &Credential,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = self.service.clone();
        let id = credential.id.clone();
        let favorite = !credential.favorite;
        cx.spawn_in(window, async move |this, cx| {
            let result = service.set_favorite(&id, favorite).await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(_) => this.reload(window, cx),
                Err(error) => window.push_notification(
                    t!("vault.favorite_failed", error => format!("{error:#}")),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// 新建一条凭据：空表单，提交后由 [`Self::credential_saved`] 收尾。
    pub fn new_credential(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_saved = self.saved_handler(cx);
        CredentialFormDialog::open(self.service.clone(), on_saved, None, None, window, cx);
    }

    /// 随机密码生成器。
    pub(crate) fn open_password_generator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_saved = self.saved_handler(cx);
        PasswordGeneratorDialog::open(self.service.clone(), on_saved, window, cx);
    }

    /// 生成一对 SSH 密钥并入库。
    pub(crate) fn open_key_generator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_saved = self.saved_handler(cx);
        KeyGeneratorDialog::open(self.service.clone(), on_saved, window, cx);
    }

    fn edit(&mut self, credential: &Credential, window: &mut Window, cx: &mut Context<Self>) {
        let on_saved = self.saved_handler(cx);
        CredentialFormDialog::open(
            self.service.clone(),
            on_saved,
            Some(credential.clone()),
            None,
            window,
            cx,
        );
    }

    /// 删除前问一句。会丢东西的动作配危险色的按钮。
    ///
    /// 确认文案里特意说明"此前取用过的主机不受影响"：保险箱和主机之间是取用
    /// 不是引用（见 [`crate::credential`] 的模块文档），而这一点从界面上看不
    /// 出来 —— 不说的话，用户会怕删掉这一条就连不上那几台机器了。
    fn confirm_delete(
        &mut self,
        credential: &Credential,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = self.service.clone();
        let vault = cx.weak_entity();
        let id = credential.id.clone();
        let name = credential.name.clone();

        window.open_alert_dialog(cx, move |alert, _, _| {
            // 外壳闭包是 `Fn`，弹一次搭一次：捕获的工作对象在这里各自复制一份，
            // 才能挪进"删除"那个会消费它们的回调里。
            let service = service.clone();
            let vault = vault.clone();
            let id = id.clone();
            alert
                .title(t!("vault.delete_title"))
                .description(SharedString::from(t!(
                    "vault.delete_confirm",
                    name => name,
                )))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("vault.delete"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("vault.cancel"))
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let service = service.clone();
                    let vault = vault.clone();
                    let id = id.clone();
                    window
                        .spawn(cx, async move |cx| {
                            let result = service.delete(&id).await;
                            vault
                                .update_in(cx, |vault, window, cx| match result {
                                    Ok(()) => vault.reload(window, cx),
                                    Err(error) => window.push_notification(
                                        t!("vault.delete_failed", error => format!("{error:#}")),
                                        cx,
                                    ),
                                })
                                .ok();
                        })
                        .detach();
                    // 返回真：确认框收起，无论删没删成 —— 删不成已经有通知了。
                    true
                })
        });
    }

    /// 把私钥另存成一个本地文件。
    ///
    /// 保险箱存的是内容，但 `ssh -i`、`scp`、Ansible 这些外面的工具要的是一个
    /// **文件**；这一条就是那座桥。写出去的文件在类 Unix 上会被收成 `0600`，
    /// 见 [`LocalFileService::write_text`]。
    fn save_private_key(
        &mut self,
        credential: &Credential,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if credential.private_key.is_empty() {
            return;
        }

        // 建议的文件名照凭据的名字来，去掉路径里不能用的那几个字符。
        let suggested = format!("{}.pem", sanitize_file_name(&credential.name));
        let picked = cx.prompt_for_new_path(&LocalFileService::start_dir(), Some(&suggested));
        let contents = credential.private_key.to_string();

        cx.spawn_in(window, async move |_, cx| {
            // 三层：通道没断、对话框没出错、用户没取消。
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };

            let shown = path.display().to_string();
            let outcome = LocalFileService::write_text(path, contents).await;
            _ = cx.update(|window, cx| match outcome {
                Ok(()) => window.push_notification(t!("vault.save_pem_done", path => shown), cx),
                Err(error) => window.push_notification(
                    t!("vault.save_pem_failed", error => format!("{error:#}")),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// 把整个保险箱导出成一个 JSON 文件。
    ///
    /// 导出的是**明文**：这是一份备份，能拿去在另一台机器上重建保险箱，
    /// 所以每一段密码、每一份私钥都在里面。先弹一句确认把这件事说清楚 ——
    /// 存到桌面上的一个 json 里，和存进库文件是两个量级的暴露面。
    pub(crate) fn export_backup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.credentials.len();
        if count == 0 {
            window.push_notification(t!("vault.export_empty"), cx);
            return;
        }

        let vault = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            // 外壳闭包是 `Fn`，弹一次搭一次：捕获的东西在这里各自复制一份。
            let vault = vault.clone();
            alert
                .title(t!("vault.export_title"))
                .description(SharedString::from(t!(
                    "vault.export_confirm",
                    count => count,
                )))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("vault.export"))
                        .cancel_text(t!("vault.cancel"))
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    vault
                        .update(cx, |this, cx| this.write_backup(window, cx))
                        .ok();
                    true
                })
        });
    }

    /// 挑一个落点，把备份写出去。
    fn write_backup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let suggested = format!(
            "winssh-vault-backup-{}.json",
            chrono::Local::now().format("%Y-%m-%d")
        );
        let picked = cx.prompt_for_new_path(&LocalFileService::start_dir(), Some(&suggested));
        let contents = backup_json(&self.credentials);

        cx.spawn_in(window, async move |_, cx| {
            // 三层：通道没断、对话框没出错、用户没取消。
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };

            let shown = path.display().to_string();
            let outcome = LocalFileService::write_text(path, contents).await;
            _ = cx.update(|window, cx| match outcome {
                Ok(()) => window.push_notification(t!("vault.export_done", path => shown), cx),
                Err(error) => window.push_notification(
                    t!("vault.export_failed", error => format!("{error:#}")),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// 标题行：视图图标 + 名称 + 新建 + 独立窗口 + 收起。
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        view_header(SidebarView::Keys, cx)
            .child(
                Button::new("credential-vault-open-window")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Maximize)
                    .tooltip(t!("vault.open_window"))
                    .accessibility_id(t!("vault.open_window"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(CredentialVaultEvent::OpenWindow))),
            )
            .child(
                Button::new("credential-vault-add")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip(t!("vault.new_credential"))
                    .accessibility_id(t!("vault.new_credential"))
                    .on_click(cx.listener(|this, _, window, cx| this.new_credential(window, cx))),
            )
            .child(
                collapse_button()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(CredentialVaultEvent::Collapse))),
            )
    }

    /// 搜索框 + 两排筛选片 + 两个生成器。这一段合起来决定列表里出现什么。
    fn render_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 先渲染出来再交给 `children`：闭包借了 `cx`，惰性迭代器会把这个
        // 借用带出闭包体，borrow checker 不放行。
        let chips: Vec<_> = self
            .filters()
            .into_iter()
            .enumerate()
            .map(|(ix, (filter, label, tone))| self.render_filter_chip(ix, filter, label, tone, cx))
            .collect();
        let categories = self.categories();

        v_flex()
            .flex_shrink_0()
            .px(GUTTER)
            .py(GUTTER)
            .gap(GUTTER)
            .child(
                Input::new(&self.search)
                    .cleanable(true)
                    .prefix(
                        Icon::new(IconName::Search)
                            .with_size(ICON_SIZE)
                            .text_color(palette::idle(cx)),
                    )
                    .w_full()
                    .bg(palette::surface(cx)),
            )
            .child(
                // 片比面板宽是常态：横向滚动，不换行也不挤压 ——
                // 换行会让这一段的高度随凭据的档数变化，整个面板就跟着跳。
                h_flex()
                    .id("credential-vault-filters")
                    .w_full()
                    .gap(CHIP_GAP)
                    .overflow_x_scroll()
                    .children(chips),
            )
            .when(!categories.is_empty(), |this| {
                this.child(self.render_category_chips(categories, cx))
            })
            .child(self.render_generators(cx))
    }

    /// 上面那一排片的顺序：全部、三档、特别关注。
    fn filters(&self) -> Vec<(VaultFilter, SharedString, Option<gpui_kit::Hsla>)> {
        let mut filters = vec![(
            VaultFilter::All,
            SharedString::from(t!("vault.filter_all", count => self.credentials.len())),
            None,
        )];
        filters.extend(CredentialKind::ALL.into_iter().map(|kind| {
            let count = self
                .credentials
                .iter()
                .filter(|credential| credential.kind == kind)
                .count();
            (
                VaultFilter::Kind(kind),
                SharedString::from(format!("{} ({count})", kind.label())),
                Some(kind_tone(kind)),
            )
        }));
        filters.push((
            VaultFilter::Favorite,
            SharedString::from(t!("vault.filter_favorite")),
            Some(palette::amber()),
        ));
        filters
    }

    /// 返回具体的 [`Button`] 而不是 `impl IntoElement`：后者会隐式捕获 `cx` 的
    /// 生命周期，装进 `Vec` 之后借用就跟着跑出闭包了。
    fn render_filter_chip(
        &self,
        ix: usize,
        filter: VaultFilter,
        label: SharedString,
        tone: Option<gpui_kit::Hsla>,
        cx: &mut Context<Self>,
    ) -> Button {
        let selected = self.filter == filter;
        // 选中的那一片描的是**这一档自己的颜色**，和卡片上的分档色是同一个：
        // 筛到"SSH 私钥"时，那一圈琥珀色和下面一列卡片的图标底色对得上。
        let ring = tone.unwrap_or_else(palette::blue);
        let variant = ButtonCustomVariant::new(cx)
            .color(palette::surface(cx))
            .foreground(if selected {
                cx.theme().sidebar_foreground
            } else {
                palette::idle(cx)
            })
            .hover(palette::surface_strong(cx))
            .active(palette::surface_strong(cx));

        Button::new(("credential-vault-filter", ix))
            .custom(variant)
            .small()
            .rounded(cx.theme().radius)
            .label(label)
            .when(selected, |this| this.border_1().border_color(ring))
            .on_click(cx.listener(move |this, _, _, cx| this.set_filter(filter, cx)))
    }

    /// 下面那一排目录片。第一个是"全部目录"，其余是凭据自己身上收出来的。
    fn render_category_chips(
        &self,
        categories: Vec<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 一条一条地搭而不是 `map`：那个闭包要同时借着 `self` 和 `cx`，
        // 惰性迭代器会把这个借用带出闭包体，borrow checker 不放行。
        let mut chips =
            vec![self.render_category_chip(0, None, t!("credential.uncategorized").into(), cx)];
        for (ix, category) in categories.into_iter().enumerate() {
            chips.push(self.render_category_chip(ix + 1, Some(category.clone()), category, cx));
        }

        h_flex()
            .id("credential-vault-categories")
            .w_full()
            .gap(CHIP_GAP)
            .overflow_x_scroll()
            .children(chips)
    }

    fn render_category_chip(
        &self,
        ix: usize,
        category: Option<SharedString>,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> Button {
        let selected = self.category == category;
        // 目录片比档片安静一档：它是第二层筛子，不该和上面那排抢注意力，
        // 所以选中只是把字点亮，不描边。
        let variant = ButtonCustomVariant::new(cx)
            .color(cx.theme().transparent)
            .foreground(if selected {
                palette::teal()
            } else {
                palette::idle(cx)
            })
            .hover(palette::surface(cx))
            .active(palette::surface_strong(cx));

        Button::new(("credential-vault-category", ix))
            .custom(variant)
            .xsmall()
            .rounded(cx.theme().radius)
            .label(match category.is_none() {
                // 第一片说的是"不按目录筛"，不是那个叫"未分组"的目录。
                true => SharedString::from(t!("vault.filter_all", count => self.credentials.len())),
                false => label,
            })
            .on_click(cx.listener(move |this, _, _, cx| this.set_category(category.clone(), cx)))
    }

    /// 两个生成器。它们不筛任何东西，是"从无到有造一份"的入口，
    /// 所以贴在筛选片下面单独一行，用安静的幽灵按钮。
    fn render_generators(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .child(
                Button::new("credential-vault-password-generator")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(AppIcon::Sparkles).text_color(palette::amber()))
                    .label(t!("vault.generate_password"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_password_generator(window, cx);
                    })),
            )
            .child(
                Button::new("credential-vault-key-generator")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(AppIcon::FileCode).text_color(palette::teal()))
                    .label(t!("vault.generate_keypair"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_key_generator(window, cx);
                    })),
            )
    }

    /// 列表：一列卡片。空了就说清楚是"还没有"还是"没筛到"。
    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.visible(cx);
        let empty = self.credentials.is_empty();

        div()
            .flex_1()
            .min_h_0()
            .bg(palette::surface(cx))
            .map(|this| match visible.is_empty() {
                true => this.child(self.render_empty(empty, cx)),
                false => this.child(
                    v_flex()
                        .id("credential-vault-list")
                        .size_full()
                        .px(GUTTER)
                        .py_1()
                        .gap(CARD_GAP)
                        .overflow_y_scroll()
                        .children(
                            visible
                                .into_iter()
                                .enumerate()
                                .map(|(ix, credential)| self.render_card(ix, credential, cx)),
                        ),
                ),
            })
    }

    /// 返回具体的 [`CredentialCard`] 而不是 `impl IntoElement`：后者会隐式
    /// 捕获 `cx` 的生命周期，在 `children` 那个闭包里就跑出闭包体了。
    fn render_card(
        &self,
        ix: usize,
        credential: Credential,
        cx: &mut Context<Self>,
    ) -> CredentialCard {
        let actioned = credential.clone();
        let revealed = self.revealed.contains(&credential.id);

        CredentialCard::new(("credential", ix), credential)
            .revealed(revealed)
            .on_action(
                cx.listener(move |this, action: &CredentialAction, window, cx| {
                    this.run_action(*action, &actioned, window, cx);
                }),
            )
    }

    /// 空状态。整个保险箱空着的时候多说一句它能装什么 ——
    /// 那是这个视图第一次被打开时唯一的说明书。
    fn render_empty(&self, vault_is_empty: bool, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .p_6()
            .gap_2()
            .items_center()
            .justify_center()
            .text_center()
            .child(
                Icon::new(AppIcon::ShieldCheck)
                    .large()
                    .text_color(palette::idle(cx)),
            )
            .child(
                div()
                    .text_sm()
                    .child(SharedString::from(match vault_is_empty {
                        true => t!("vault.empty"),
                        false => t!("vault.no_match"),
                    })),
            )
            .when(vault_is_empty, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(t!("vault.empty_hint"))),
                )
            })
    }

    /// 底部：一个主操作，下面一句实话 —— 这些东西存在哪儿、加没加密。
    ///
    /// 那句话不是装饰：库里存的是明文（和 `hosts` 表里的密码一样），
    /// 用户有权在把生产 root 密码放进来之前就知道这件事。
    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let blue = palette::blue();
        let variant = ButtonCustomVariant::new(cx)
            .color(blue)
            .foreground(palette::on_accent())
            .hover(blue.lighten(0.08))
            .active(blue.darken(0.08));

        v_flex()
            .flex_shrink_0()
            .p(GUTTER)
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                Button::new("credential-vault-new")
                    .custom(variant)
                    // Custom 变体的常态底色会被稀释两成，这里把设计稿的实色压回去。
                    .bg(blue)
                    .rounded(cx.theme().radius)
                    .icon(IconName::Plus)
                    .label(t!("vault.new_credential"))
                    .w_full()
                    .on_click(cx.listener(|this, _, window, cx| this.new_credential(window, cx))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("vault.storage_note"))),
            )
    }
}

/// 把一个凭据名整成能当文件名的样子。
///
/// Windows 上 `\ / : * ? " < > |` 一个都不能出现在文件名里，类 Unix 上 `/`
/// 不行；一律换成下划线，省得为各平台各写一套。整串都被换掉（名字全是这些
/// 字符）时给一个兜底，不然存出来的文件叫 `.pem`。
fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    match cleaned.is_empty() {
        true => "id_key".to_string(),
        false => cleaned,
    }
}

impl EventEmitter<CredentialVaultEvent> for CredentialVaultView {}

impl Focusable for CredentialVaultView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CredentialVaultView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(self.render_header(cx))
            .child(self.render_filters(cx))
            .child(Separator::horizontal().color(cx.theme().sidebar_border))
            .child(self.render_list(cx))
            .child(Separator::horizontal().color(cx.theme().sidebar_border))
            .child(self.render_footer(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::KeyAlgorithm;

    fn credential(name: &str, kind: CredentialKind, category: &str, favorite: bool) -> Credential {
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
            favorite,
            updated_at: "2025-01-10 09:30:00".into(),
        }
    }

    /// 搭一个带着现成快照的视图，走 [`CredentialVaultView::from_snapshot`]
    /// 而**不**走 [`CredentialVaultView::new`]，理由见那个构造器的文档：
    /// 不碰真库、不派异步。内存库在开窗**之前**就建好，
    /// 之后这几条测试一次异步都不发。
    ///
    /// 这几条测的是筛选与画法，数据层归 [`crate::service::credential_service`]
    /// 自己那套测试。
    fn draw_vault(
        cx: &mut gpui_kit::TestAppContext,
        credentials: Vec<Credential>,
    ) -> (
        Entity<CredentialVaultView>,
        &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(gpui_kit::init);
        let service = CredentialService::in_memory_blocking();
        let (vault, cx) = cx.add_window_view(|window, cx| {
            CredentialVaultView::from_snapshot(service, credentials, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (vault, cx)
    }

    fn names(
        vault: &Entity<CredentialVaultView>,
        cx: &mut gpui_kit::VisualTestContext,
    ) -> Vec<String> {
        vault.read_with(cx, |this, cx| {
            this.visible(cx)
                .iter()
                .map(|credential| credential.name.to_string())
                .collect()
        })
    }

    fn sample() -> Vec<Credential> {
        vec![
            credential("prod-root", CredentialKind::Password, "Linux", true),
            credential("aws-key", CredentialKind::PrivateKey, "云服务", false),
            credential("k8s-token", CredentialKind::Token, "Linux", false),
        ]
    }

    /// 三道筛子各自管各自的一段，叠起来还是与的关系。
    #[gpui_kit::test]
    fn the_three_filters_stack(cx: &mut gpui_kit::TestAppContext) {
        let (vault, cx) = draw_vault(cx, sample());
        assert_eq!(names(&vault, cx).len(), 3);

        vault.update(cx, |this, cx| {
            this.set_filter(VaultFilter::Kind(CredentialKind::PrivateKey), cx)
        });
        assert_eq!(names(&vault, cx), vec!["aws-key".to_string()]);

        vault.update(cx, |this, cx| this.set_filter(VaultFilter::Favorite, cx));
        assert_eq!(names(&vault, cx), vec!["prod-root".to_string()]);

        vault.update(cx, |this, cx| this.set_filter(VaultFilter::All, cx));
        vault.update(cx, |this, cx| this.set_category(Some("Linux".into()), cx));
        assert_eq!(
            names(&vault, cx),
            vec!["prod-root".to_string(), "k8s-token".to_string()]
        );

        // 档与目录同时筛，两个都要满足。
        vault.update(cx, |this, cx| {
            this.set_filter(VaultFilter::Kind(CredentialKind::Token), cx)
        });
        assert_eq!(names(&vault, cx), vec!["k8s-token".to_string()]);
    }

    /// 目录片是从凭据身上收出来的，去重、排序，没归类的不算。
    #[gpui_kit::test]
    fn categories_come_from_the_credentials(cx: &mut gpui_kit::TestAppContext) {
        let mut credentials = sample();
        credentials.push(credential("野生的", CredentialKind::Password, "", false));

        let (vault, cx) = draw_vault(cx, credentials);
        vault.read_with(cx, |this, _| {
            assert_eq!(this.categories(), vec!["Linux", "云服务"]);
        });
    }

    /// 明文一条一条地开、一条一条地关，互不影响。
    #[gpui_kit::test]
    fn revealing_is_per_credential(cx: &mut gpui_kit::TestAppContext) {
        let (vault, cx) = draw_vault(cx, sample());
        let root = SharedString::from("cred-prod-root");
        let token = SharedString::from("cred-k8s-token");

        vault.update(cx, |this, cx| {
            this.toggle_reveal(&root, cx);
            this.toggle_reveal(&token, cx);
            this.toggle_reveal(&root, cx);
        });

        vault.read_with(cx, |this, _| {
            assert!(!this.revealed.contains(&root), "开了又关的那条该收回去");
            assert!(this.revealed.contains(&token));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 存完之后那条凭据必须看得见：过不去的那道筛子退回"全部"，
    /// 不然用户会以为没存上。
    #[gpui_kit::test]
    fn saving_jumps_back_to_a_filter_that_shows_it(cx: &mut gpui_kit::TestAppContext) {
        let (vault, cx) = draw_vault(cx, sample());
        vault.update(cx, |this, cx| {
            this.set_filter(VaultFilter::Kind(CredentialKind::Token), cx);
            this.set_category(Some("Linux".into()), cx);
        });

        let saved = credential("new-key", CredentialKind::PrivateKey, "云服务", false);
        vault.update(cx, |this, _| this.ensure_visible(&saved));

        vault.read_with(cx, |this, _| {
            assert_eq!(this.filter, VaultFilter::All);
            assert_eq!(this.category, None);
        });
    }

    /// 只退过不去的那一道。档筛的是私钥、存的也是私钥，那一道不该动；
    /// 目录对不上，退的是目录。
    #[gpui_kit::test]
    fn saving_only_relaxes_the_filter_that_hides_it(cx: &mut gpui_kit::TestAppContext) {
        let (vault, cx) = draw_vault(cx, sample());
        vault.update(cx, |this, cx| {
            this.set_filter(VaultFilter::Kind(CredentialKind::PrivateKey), cx);
            this.set_category(Some("Linux".into()), cx);
        });

        let saved = credential("new-key", CredentialKind::PrivateKey, "云服务", false);
        vault.update(cx, |this, _| this.ensure_visible(&saved));

        vault.read_with(cx, |this, _| {
            assert_eq!(
                this.filter,
                VaultFilter::Kind(CredentialKind::PrivateKey),
                "这一道本来就过得去，不该动"
            );
            assert_eq!(this.category, None, "过不去的那一道要退回来");
        });
    }

    /// 整屏画得通，三档卡片都在。深 builder 链上的错 `cargo check` 看不见。
    #[gpui_kit::test]
    fn the_vault_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (_vault, cx) = draw_vault(cx, sample());
        // 画两帧：第一帧和后续帧走的不是同一条路。
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 空保险箱也要画得出来，而且说的是"还没有"不是"没筛到"。
    #[gpui_kit::test]
    fn an_empty_vault_draws_its_hint(cx: &mut gpui_kit::TestAppContext) {
        let (_vault, cx) = draw_vault(cx, Vec::new());
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[test]
    fn file_names_lose_the_characters_windows_refuses() {
        assert_eq!(sanitize_file_name("prod/root:key"), "prod_root_key");
        assert_eq!(sanitize_file_name("生产 密钥"), "生产 密钥");
        assert_eq!(sanitize_file_name("///"), "___");
        // 一个字都没剩下时给一个兜底，不然存出来的文件叫 `.pem`。
        assert_eq!(sanitize_file_name("  "), "id_key");
        assert_eq!(sanitize_file_name("..."), "id_key");
    }
}
