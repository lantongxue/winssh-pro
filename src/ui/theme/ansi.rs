//! 终端网格的配色：ANSI 那 16 色、256 色索引与真彩色怎么落到具体的像素颜色。
//!
//! 主题（组件库那套 shadcn neutral）里没有终端色 —— 它是给控件用的，
//! 只有前景、底色、边框那几档。终端要的是另一套东西：程序用 `\x1b[31m` 说
//! "红"，这一套得回答"红是哪个红"。所以这里固定一组，取自 VS Code 的
//! Dark+ / Light+，与 [`crate::ui::theme::palette`] 里的品牌色是同一套观感。
//!
//! 明暗两套都给全了：终端的底色跟着主题走，前景色和 16 色也得跟着换，
//! 不然浅色模式下一屏字都糊在白底上。

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Hsla, Rgba, rgb};

use crate::ui::theme::palette;

use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb};

/// 终端用到的一整套颜色。每帧取一次，逐格查表 —— 一屏几千个格子，
/// 每个都回头问一次主题的话，`cx.theme()` 那几层解引用就成了热点。
#[derive(Clone, Copy, Debug)]
pub struct AnsiPalette {
    /// 标准 8 色，再跟上高亮 8 色。索引与 ANSI 编号一致。
    colors: [Hsla; 16],
    foreground: Hsla,
    background: Hsla,
    /// 光标那一块的底色。
    cursor: Hsla,
    /// 选中那一段的底色。
    selection: Hsla,
    /// 搜索命中的底色，以及当前那一个的。
    match_hit: Hsla,
    match_current: Hsla,
}

/// Dark+ 的 16 色：黑、红、绿、黄、蓝、洋红、青、白，然后是同顺序的高亮 8 色。
const DARK: [u32; 16] = [
    0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, 0x666666,
    0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
];

/// Light+ 的 16 色。同顺序。
const LIGHT: [u32; 16] = [
    0x000000, 0xcd3131, 0x00bc00, 0x949800, 0x0451a5, 0xbc05bc, 0x0598bc, 0x555555, 0x666666,
    0xcd3131, 0x14ce14, 0xb5ba00, 0x0451a5, 0xbc05bc, 0x0598bc, 0xa5a5a5,
];

/// 暗色下终端的正文色。比主题前景（近白）柔一档 —— 一屏满是等宽字，
/// 纯白读久了会晃眼，这也是各家终端的默认做法。
const DARK_FOREGROUND: u32 = 0xd4d4d4;

impl AnsiPalette {
    /// 按当前主题的明暗取一套。底色直接用主题底色：终端是面板的内容区，
    /// 它和四周的 chrome 应该是同一个底。
    pub fn new(cx: &App) -> Self {
        let dark = cx.theme().mode.is_dark();
        let colors = if dark { DARK } else { LIGHT };

        Self {
            colors: colors.map(|color| rgb(color).into()),
            foreground: if dark {
                rgb(DARK_FOREGROUND).into()
            } else {
                cx.theme().foreground
            },
            background: cx.theme().background,
            cursor: cx.theme().foreground,
            selection: cx.theme().selection,
            // 琥珀色，各家编辑器找东西时的老规矩，而且终端的 16 色里没有
            // 一个是这个色相 —— 高亮不会和程序自己的配色撞上。
            //
            // 两档都是半透明的：底下压着的可能是程序画的底色（`ls` 的目录、
            // diff 的红绿），盖死了就分不出命中落在什么东西上。
            match_hit: palette::amber().opacity(0.28),
            match_current: palette::amber().opacity(0.62),
        }
    }

    pub fn cursor(&self) -> Hsla {
        self.cursor
    }

    /// 终端的默认底色。半透明的搜索高亮要合成到它上面。
    pub fn background(&self) -> Hsla {
        self.background
    }

    pub fn selection(&self) -> Hsla {
        self.selection
    }

    /// 搜索命中的底色。
    pub fn match_hit(&self) -> Hsla {
        self.match_hit
    }

    /// 当前停在的那一个命中。比其余的重，一眼能从一屏黄块里找出来
    /// —— "还有 17 个"和"你现在在第 3 个上"是两件事。
    pub fn match_current(&self) -> Hsla {
        self.match_current
    }

    /// 一个 ANSI 颜色对应的像素颜色。
    pub fn color(&self, color: AnsiColor) -> Hsla {
        match color {
            AnsiColor::Named(named) => self.named(named),
            AnsiColor::Spec(rgb) => spec(rgb),
            AnsiColor::Indexed(index) => self.indexed(index),
        }
    }

    fn named(&self, named: NamedColor) -> Hsla {
        match named {
            NamedColor::Black => self.colors[0],
            NamedColor::Red => self.colors[1],
            NamedColor::Green => self.colors[2],
            NamedColor::Yellow => self.colors[3],
            NamedColor::Blue => self.colors[4],
            NamedColor::Magenta => self.colors[5],
            NamedColor::Cyan => self.colors[6],
            NamedColor::White => self.colors[7],
            NamedColor::BrightBlack => self.colors[8],
            NamedColor::BrightRed => self.colors[9],
            NamedColor::BrightGreen => self.colors[10],
            NamedColor::BrightYellow => self.colors[11],
            NamedColor::BrightBlue => self.colors[12],
            NamedColor::BrightMagenta => self.colors[13],
            NamedColor::BrightCyan => self.colors[14],
            NamedColor::BrightWhite => self.colors[15],
            NamedColor::Foreground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            // 暗色（SGR 2）没有独立的一套色板：alacritty 只把它当"标准 8 色再压暗"，
            // 而压暗这件事由 [`crate::ui::elements::TerminalGridElement`] 调 alpha 完成。
            NamedColor::DimBlack => self.colors[0],
            NamedColor::DimRed => self.colors[1],
            NamedColor::DimGreen => self.colors[2],
            NamedColor::DimYellow => self.colors[3],
            NamedColor::DimBlue => self.colors[4],
            NamedColor::DimMagenta => self.colors[5],
            NamedColor::DimCyan => self.colors[6],
            NamedColor::DimWhite => self.colors[7],
            NamedColor::BrightForeground => self.colors[15],
            NamedColor::DimForeground => self.foreground.opacity(0.7),
        }
    }

    /// 256 色索引。前 16 个就是上面那 16 色，之后是 xterm 定下来的
    /// 6×6×6 色立方与 24 级灰阶。
    fn indexed(&self, index: u8) -> Hsla {
        match index {
            0..=15 => self.colors[index as usize],
            16..=231 => {
                let index = index - 16;
                // 每个通道 6 档，档位值是 xterm 的 `256colres.pl` 里那一串。
                let level = |channel: u8| match channel {
                    0 => 0,
                    other => other * 40 + 55,
                };
                spec(Rgb {
                    r: level(index / 36),
                    g: level((index % 36) / 6),
                    b: level(index % 6),
                })
            }
            232..=255 => {
                let value = (index - 232) * 10 + 8;
                spec(Rgb {
                    r: value,
                    g: value,
                    b: value,
                })
            }
        }
    }
}

/// 真彩色（`\x1b[38;2;r;g;bm`）：程序给的就是最终颜色，照搬。
fn spec(color: Rgb) -> Hsla {
    Rgba {
        r: color.r as f32 / 255.,
        g: color.g as f32 / 255.,
        b: color.b as f32 / 255.,
        a: 1.,
    }
    .into()
}
