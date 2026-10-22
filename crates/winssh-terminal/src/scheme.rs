//! 终端配色方案：一套 ANSI 16 色，加上前景、底色与光标。
//!
//! 和 [`crate::font`] 一样，这是一份"每帧都要读"的偏好：库是正本，
//! 进程里留一份缓存，启动时由 [`crate::service::SettingService::prime`] 灌一次。
//! 落库存的是方案 id 那一个词，不是整张色表 —— 色表在代码里，换了版本跟着
//! 代码走；存整张表的话，改一个色值还得去迁移用户那一行。
//!
//! **出厂档是"跟随界面主题"**（id 为空串），也就是现在这套 VS Code Dark+ /
//! Light+：界面切深浅，终端跟着切。挑了具体方案之后终端就自己说了算 ——
//! Dracula 在浅色界面下仍旧是 Dracula，各家终端都是这个做法。
//!
//! 色值取自 tty7（`refs/tty7/src/ui/presets.rs`，Apache-2.0）里那份已经按
//! 对比度校过的表，它们各自的上游是 Dracula、One Dark、Nord、Gruvbox、
//! Tokyo Night、Catppuccin、Rosé Pine 这几个项目。
//!
//! **不做 tty7 那一层"高亮色补偿"。** 它把对比度不够的高亮 8 色朝前景拉到
//! 4.5:1，为的是 PSReadLine 那类拿高亮色画参数的程序。代价是 Dracula 的
//! 注释灰（bright black `#555555`，1.12:1）会被拉成一个接近正文的亮灰 ——
//! 那个低对比度正是它的用意。挑了 Dracula 的人要的是 Dracula，所以这里
//! 照着作者写的画，改由 [`tests::bright_black_is_never_the_background`]
//! 挡住真正会出事的那一种：高亮黑和底色一模一样，`\x1b[90m` 整段隐形
//! （Solarized 就是这样，所以它不在这张表里）。

use std::sync::{LazyLock, RwLock};

use gpui_kit::SharedString;

/// 一套配色。
pub struct Scheme {
    /// 落库存的就是这个词。
    pub id: &'static str,
    /// 界面上的名字。是专有名词，不进 i18n。
    pub name: &'static str,
    pub background: u32,
    pub foreground: u32,
    /// 光标块的底色。`None` 表示用前景色 —— 绝大多数方案就是这样。
    pub caret: Option<u32>,
    /// 标准 8 色接高亮 8 色，索引与 ANSI 编号一致。
    pub ansi16: [u32; 16],
}

impl Scheme {
    /// 光标块的底色。
    pub fn cursor(&self) -> u32 {
        self.caret.unwrap_or(self.foreground)
    }
}

/// 内置的几套配色。深色在前、浅色在后，下拉里就照这个顺序摆。
pub static PRESETS: [Scheme; 10] = [
    Scheme {
        id: "dracula",
        name: "Dracula",
        background: 0x282a36,
        foreground: 0xf8f8f2,
        caret: None,
        ansi16: [
            0x000000, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xbbbbbb,
            0x555555, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xcaa9fa, 0xff79c6, 0x8be9fd, 0xffffff,
        ],
    },
    Scheme {
        id: "one_dark_pro",
        name: "One Dark Pro",
        background: 0x282c34,
        foreground: 0xabb2bf,
        caret: None,
        ansi16: [
            0x3f4451, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
            0x5c6370, 0xff616e, 0xa5e075, 0xf0a45d, 0x4dc4ff, 0xde73ff, 0x4cd1e0, 0xe6e6e6,
        ],
    },
    Scheme {
        id: "nord",
        name: "Nord",
        background: 0x2e3440,
        foreground: 0xd8dee9,
        caret: None,
        ansi16: [
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
            0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ],
    },
    Scheme {
        id: "gruvbox_dark",
        name: "Gruvbox Dark",
        background: 0x282828,
        foreground: 0xebdbb2,
        caret: None,
        ansi16: [
            0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
            0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
        ],
    },
    Scheme {
        id: "tokyo_night",
        name: "Tokyo Night",
        background: 0x1a1b26,
        foreground: 0xc0caf5,
        caret: None,
        ansi16: [
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
            0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
        ],
    },
    Scheme {
        id: "catppuccin_mocha",
        name: "Catppuccin Mocha",
        background: 0x1e1e2e,
        foreground: 0xcdd6f4,
        // Rosewater。Catppuccin 自己的终端规格就是拿它当光标色的，
        // 退回前景会画成一个偏蓝的块。
        caret: Some(0xf5e0dc),
        ansi16: [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
            0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
        ],
    },
    Scheme {
        id: "rose_pine",
        name: "Rosé Pine",
        background: 0x191724,
        foreground: 0xe0def4,
        caret: None,
        ansi16: [
            0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
            0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
        ],
    },
    Scheme {
        id: "one_light",
        name: "One Light",
        background: 0xfafafa,
        foreground: 0x383a42,
        caret: None,
        ansi16: [
            0x383a42, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0xa0a1a7,
            0x696c77, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0xfafafa,
        ],
    },
    Scheme {
        id: "catppuccin_latte",
        name: "Catppuccin Latte",
        background: 0xeff1f5,
        foreground: 0x4c4f69,
        caret: None,
        ansi16: [
            0xbcc0cc, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0x5c5f77,
            0xacb0be, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0x6c6f85,
        ],
    },
    Scheme {
        id: "rose_pine_dawn",
        name: "Rosé Pine Dawn",
        background: 0xfaf4ed,
        foreground: 0x575279,
        caret: None,
        ansi16: [
            0xf2e9e1, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
            0x9893a5, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
        ],
    },
];

/// 按 id 找一套配色。空串（跟随界面主题）与认不出来的 id 都是 `None`。
pub fn find(id: &str) -> Option<&'static Scheme> {
    PRESETS.iter().find(|scheme| scheme.id == id)
}

/// 选中的配色 id 在进程里的那一份缓存。正本在库里（`settings` 表）。
static SELECTED: LazyLock<RwLock<SharedString>> =
    LazyLock::new(|| RwLock::new(SharedString::default()));

/// 现在选的是哪一套。空串是跟随界面主题。
pub fn global() -> SharedString {
    SELECTED
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 现在这一套配色，`None` 表示跟随界面主题。渲染读的是这一个。
pub fn active() -> Option<&'static Scheme> {
    find(&global())
}

/// 换掉选中的配色。认不出来的 id 一律当成"跟随界面主题" —— 落库那一行是
/// 手改得到的，而一个查不到的 id 只能这么处置。
pub fn set_global(id: &str) {
    let id = find(id).map(|scheme| scheme.id).unwrap_or_default();
    *SELECTED
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = SharedString::from(id);
}

/// 测试用：把"这条测试要动全局配色"串起来，理由同
/// [`crate::proxy::exclusive_for_test`]。守卫掉地时放回跟随界面主题。
// 不用 `#[cfg(test)]` 门：上层 crate（根包）的测试也要用它，而 `#[cfg(test)]`
// 的东西链进下游就没了。`#[doc(hidden)]` 不进文档，正式构建里没人调。
#[doc(hidden)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global("");
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// 存进 `settings` 表的就是 id 本身，不套 JSON —— 只有一个值。
pub fn encode(id: &str) -> String {
    id.to_string()
}

/// 从 `settings` 表那一行读回来。认不出来的当跟随界面主题。
pub fn decode(raw: &str) -> SharedString {
    let id = raw.trim();
    match find(id) {
        Some(scheme) => SharedString::from(scheme.id),
        None => {
            if !id.is_empty() {
                log::warn!("认不出配色方案 {id:?}，这次运行跟随界面主题");
            }
            SharedString::default()
        }
    }
}

/// 按比例把 `b` 兑进 `a`。`t` 是 `b` 占的份额。
#[cfg(test)]
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let channel = |shift: u32| {
        let (from, to) = ((a >> shift & 0xff) as f32, (b >> shift & 0xff) as f32);
        (from + (to - from) * t).round().clamp(0., 255.) as u32
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

#[cfg(test)]
#[path = "tests/terminal/scheme.rs"]
mod tests;
