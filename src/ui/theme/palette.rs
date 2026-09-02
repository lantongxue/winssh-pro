//! 界面的品牌色与表面层级。
//!
//! 主题（gpui-component 那套 shadcn neutral）只提供一组中性色 —— 暗色下
//! `primary` 是近白色，没有留给品牌色的语义槽位。设计稿里那几个彩色是产品
//! 自己的语义色，只能在这里固定下来。结构色（底色、边框、弱化前景）仍然
//! 全部从主题取，明暗模式才跟得住。
//!
//! 表面层级也放在这里：设计稿把 chrome（面板头尾、活动栏）画得比内容区暗
//! 一档，主题里没有这一档，用强调色压到很低叠在 `sidebar` 上得到同样的关系。
//! 因此这些函数返回的都是半透明色，靠叠在父级底色上生效。

use gpui::{App, Hsla, rgb};
use gpui_component::ActiveTheme as _;

/// 「当前所在」：活动栏里选中的那个图标。
pub fn teal() -> Hsla {
    rgb(0x43c1a0).into()
}

/// 「位置与主操作」：选中指示条、主按钮、选中的筛选片描边。
pub fn blue() -> Hsla {
    rgb(0x0c64c0).into()
}

/// 「分组与即时动作」：分组文件夹、快速连接。
pub fn coral() -> Hsla {
    rgb(0xc27e65).into()
}

/// 「配置与编辑」：菜单里编辑那一项的图标。比 [`blue`] 浅一档 ——
/// `blue` 是压着白字的实底，这里是暗底上的一个小图标，同一个蓝就看不见了。
pub fn sky() -> Hsla {
    rgb(0x4c8dc0).into()
}

/// 「这台主机上有活着的会话」：主机卡片名字前面那个点。
/// 它说的是“我开着一个终端”，跟网络通不通是两件事。
pub fn magenta() -> Hsla {
    rgb(0xe42b87).into()
}

/// 「网络可达」：主机卡片上的延迟指示。
pub fn green() -> Hsla {
    rgb(0x29c231).into()
}

/// 「暂停与等待」：暂停状态的状态药丸与"暂停全部"。
/// 蓝色是"在动"，琥珀色是"停在半路等人"—— 两眼要分得开。
pub fn amber() -> Hsla {
    rgb(0xd9a03c).into()
}

/// 落在品牌色实底上的文字与图标。固定近白色：主题前景在浅色模式下会翻成
/// 深色，压在蓝底上就看不见了。
pub fn on_accent() -> Hsla {
    rgb(0xffffff).into()
}

/// 安静的前景：未选中的图标、次要文字、计数。主题弱化前景再压一档。
pub fn idle(cx: &App) -> Hsla {
    cx.theme().muted_foreground.opacity(0.7)
}

/// 内容表面：比 chrome 亮一档，承载列表、输入框与筛选片，也用作悬停高亮。
pub fn surface(cx: &App) -> Hsla {
    cx.theme().sidebar_accent.opacity(0.4)
}

/// 选中的表面：比悬停再实一点，用在活动栏选中的药丸上。
pub fn surface_strong(cx: &App) -> Hsla {
    cx.theme().sidebar_accent.opacity(0.6)
}

/// 凹陷表面：落回 chrome 那一档。用在内容表面上的计数角标，让它读起来是嵌进去的。
pub fn recessed(cx: &App) -> Hsla {
    cx.theme().sidebar
}

/// 文件列表里被选中的那一行。和悬停同色 —— 选中不该比"鼠标正好停在这儿"更响，
/// 多选时也就不会有一片抢眼的色块。表格自己那层选中样式已经在
/// [`crate::ui::theme::appearance`] 里按透明关掉了，选中底色由文件列表自己画。
pub fn selected_row(cx: &App) -> Hsla {
    cx.theme().table_hover
}
