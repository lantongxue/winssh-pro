//! 制表符与 powerline 分隔符的自绘。
//!
//! 终端里的边框（`─│┌┐└┘├┤┬┴┼`、双线、圆角、虚线）、块元素（`▁▂▃█░▒▓`）
//! 与 powerline 三角（`\u{e0b0}` 那一串），**不交给字体**，按格子的矩形现场画出来。
//!
//! 为什么必须这么做：字体里的这些字形是照着字体自己的 em 盒设计的，而终端的
//! 格子是"字号 × 行距"算出来的另一个尺寸。行距一放开（我们是 1.4），gpui 的
//! `paint_line` 会把字形在行盒里居中（`padding_top = (line_height - ascent -
//! descent) / 2`），上下两格的 `│` 之间就空出一条缝，竖边框看着是虚线；横向
//! 同理，`─` 按字体的步进画在略宽的格子里，相邻两格之间渗一条缝。
//!
//! 换成按 `bounds`（就是那一格的矩形）画之后，竖线永远从格子顶画到格子底、
//! 横线永远从左边画到右边，行距和格宽取什么值都接得上。alacritty 的
//! `builtin_box_drawing`（`refs/alacritty/alacritty/src/renderer/text/builtin_font.rs`）
//! 做的是同一件事 —— 但它在 `alacritty` 那个二进制 crate 的渲染器里，
//! 我们只依赖 `alacritty_terminal`（解析 + 缓冲 + PTY），拿不到。
//!
//! 实现照搬自 tty7 的 `src/terminal/boxdraw.rs` 与 `element.rs` 里的 powerline
//! 部分（<https://github.com/l0ng-ai/tty7>，Apache-2.0），它是同一套
//! gpui + alacritty_terminal 组合下已经验证过的版本。
//!
//! 覆盖范围是 U+2500–U+259F 全段与 U+E0B0/B2/B4/B6/B8/BA/BC/BE。落在外面的
//! 字符（盲文 U+2800 那一段、六分块 U+1FB00）返回 `None`，照旧走字体。

use gpui_kit::{Bounds, Pixels, Point, point, px, size};

/// 一笔要画的东西。三种：实心矩形、按 alpha 打薄的矩形（`░▒▓`）、路径（圆角、
/// 斜线、powerline）。
pub(crate) enum Ink {
    Rect(Bounds<Pixels>),
    Shade(Bounds<Pixels>, f32),
    Path(gpui_kit::Path<Pixels>),
}

/// 这个字符能不能自绘；能的话给出该画的几笔。
///
/// `bounds` 是这一格的矩形，`scale` 是 `window.scale_factor()`（笔画要对齐到
/// 物理像素，否则同一条线在不同列上会一列 1px 一列 2px），`fg_alpha` 只有
/// powerline 用得着 —— 见 [`powerline_solid_edge`]。
pub(crate) fn ink(c: char, bounds: Bounds<Pixels>, scale: f32, fg_alpha: f32) -> Option<Vec<Ink>> {
    glyph(c, bounds, scale).or_else(|| powerline(c, bounds, scale, fg_alpha))
}

/// 制表符与块元素：U+2500–U+259F。
pub(crate) fn glyph(c: char, bounds: Bounds<Pixels>, scale: f32) -> Option<Vec<Ink>> {
    if !('\u{2500}'..='\u{259f}').contains(&c) {
        return None;
    }
    let g = Cell::new(&bounds, scale);
    if let Some((u, d, l, r)) = arms_of(c) {
        return Some(g.arms(u, d, l, r));
    }
    g.doubles(c)
        .or_else(|| g.rounded(c))
        .or_else(|| g.dashed(c))
        .or_else(|| g.diagonal(c))
        .or_else(|| g.blocks(c))
}

#[derive(Clone, Copy, PartialEq)]
enum Arm {
    None,
    Light,
    Heavy,
}

struct Cell {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    cx: f32,
    cy: f32,
    t: f32,
    scale: f32,
}

/// 细笔画的厚度。先按格宽的 15% 取一个逻辑像素的整数，再对齐到物理像素 ——
/// 两步都取整是为了让每一列的竖线粗细完全一致，不然格宽是小数时会一列粗一列细。
fn light_thickness(cell_width: f32, scale: f32) -> f32 {
    let logical = (cell_width * 0.15).round().max(1.);
    (logical * scale).round().max(1.) / scale
}

impl Cell {
    fn new(b: &Bounds<Pixels>, scale: f32) -> Self {
        let x0 = b.origin.x.as_f32();
        let y0 = b.origin.y.as_f32();
        let x1 = x0 + b.size.width.as_f32();
        let y1 = y0 + b.size.height.as_f32();
        let scale = scale.max(0.1);
        Cell {
            x0,
            y0,
            x1,
            y1,
            cx: (x0 + x1) / 2.,
            cy: (y0 + y1) / 2.,
            t: light_thickness(x1 - x0, scale),
            scale,
        }
    }

    fn snap(&self, v: f32) -> f32 {
        (v * self.scale).round() / self.scale
    }

    fn rectb(&self, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        let (sx0, sy0) = (self.snap(x), self.snap(y));
        let (sx1, sy1) = (self.snap(x + w), self.snap(y + h));
        Bounds::new(point(px(sx0), px(sy0)), size(px(sx1 - sx0), px(sy1 - sy0)))
    }

    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> Ink {
        Ink::Rect(self.rectb(x, y, w, h))
    }

    fn stroke_px(&self, w: f32) -> f32 {
        (w * self.scale).round().max(1.) / self.scale
    }

    fn vstroke(&self, x: f32, w: f32, ya: f32, yb: f32) -> Ink {
        let (x0, y0, y1) = (self.snap(x - w / 2.), self.snap(ya), self.snap(yb));
        Ink::Rect(Bounds::new(
            point(px(x0), px(y0)),
            size(px(self.stroke_px(w)), px(y1 - y0)),
        ))
    }

    fn hstroke(&self, y: f32, w: f32, xa: f32, xb: f32) -> Ink {
        let (y0, x0, x1) = (self.snap(y - w / 2.), self.snap(xa), self.snap(xb));
        Ink::Rect(Bounds::new(
            point(px(x0), px(y0)),
            size(px(x1 - x0), px(self.stroke_px(w))),
        ))
    }

    /// 十字的四条臂。每条臂从格子的边画到中心再多伸 `m`（最粗那条臂的一半），
    /// 交叉处才不会缺一块。
    fn arms(&self, u: Arm, d: Arm, l: Arm, r: Arm) -> Vec<Ink> {
        let w = |a: Arm| match a {
            Arm::None => 0.,
            Arm::Light => self.t,
            Arm::Heavy => self.t * 2.,
        };
        let (wu, wd, wl, wr) = (w(u), w(d), w(l), w(r));
        let m = wu.max(wd).max(wl).max(wr) / 2.;
        let mut ink = Vec::new();
        if wu > 0. {
            ink.push(self.vstroke(self.cx, wu, self.y0, self.cy + m));
        }
        if wd > 0. {
            ink.push(self.vstroke(self.cx, wd, self.cy - m, self.y1));
        }
        if wl > 0. {
            ink.push(self.hstroke(self.cy, wl, self.x0, self.cx + m));
        }
        if wr > 0. {
            ink.push(self.hstroke(self.cy, wr, self.cx - m, self.x1));
        }
        ink
    }

    /// 双线：U+2550–U+256C。两条平行的细线，间距 `d`，交叉处按方位留出开口。
    fn doubles(&self, c: char) -> Option<Vec<Ink>> {
        let t = self.t;
        let h = t / 2.;
        let d = (t * 1.5).max(2.0);
        let (x0, x1, y0, y1, cx, cy) = (self.x0, self.x1, self.y0, self.y1, self.cx, self.cy);
        let (va, vb) = (cx - d, cx + d);
        let (ha, hb) = (cy - d, cy + d);
        let v = |x: f32, ya: f32, yb: f32| self.vstroke(x, t, ya, yb);
        let hz = |y: f32, xa: f32, xb: f32| self.hstroke(y, t, xa, xb);
        Some(match c {
            '═' => vec![hz(ha, x0, x1), hz(hb, x0, x1)],
            '║' => vec![v(va, y0, y1), v(vb, y0, y1)],
            '╒' => vec![hz(ha, cx - h, x1), hz(hb, cx - h, x1), v(cx, ha - h, y1)],
            '╓' => vec![hz(cy, va - h, x1), v(va, cy - h, y1), v(vb, cy - h, y1)],
            '╔' => vec![
                v(va, ha - h, y1),
                hz(ha, va - h, x1),
                v(vb, hb - h, y1),
                hz(hb, vb - h, x1),
            ],
            '╕' => vec![hz(ha, x0, cx + h), hz(hb, x0, cx + h), v(cx, ha - h, y1)],
            '╖' => vec![hz(cy, x0, vb + h), v(va, cy - h, y1), v(vb, cy - h, y1)],
            '╗' => vec![
                v(vb, ha - h, y1),
                hz(ha, x0, vb + h),
                v(va, hb - h, y1),
                hz(hb, x0, va + h),
            ],
            '╘' => vec![v(cx, y0, hb + h), hz(ha, cx - h, x1), hz(hb, cx - h, x1)],
            '╙' => vec![v(va, y0, cy + h), v(vb, y0, cy + h), hz(cy, va - h, x1)],
            '╚' => vec![
                v(va, y0, hb + h),
                hz(hb, va - h, x1),
                v(vb, y0, ha + h),
                hz(ha, vb - h, x1),
            ],
            '╛' => vec![v(cx, y0, hb + h), hz(ha, x0, cx + h), hz(hb, x0, cx + h)],
            '╜' => vec![v(va, y0, cy + h), v(vb, y0, cy + h), hz(cy, x0, vb + h)],
            '╝' => vec![
                v(vb, y0, hb + h),
                hz(hb, x0, vb + h),
                v(va, y0, ha + h),
                hz(ha, x0, va + h),
            ],
            '╞' => vec![v(cx, y0, y1), hz(ha, cx - h, x1), hz(hb, cx - h, x1)],
            '╟' => vec![v(va, y0, y1), v(vb, y0, y1), hz(cy, vb - h, x1)],
            '╠' => vec![
                v(va, y0, y1),
                v(vb, y0, ha + h),
                v(vb, hb - h, y1),
                hz(ha, vb - h, x1),
                hz(hb, vb - h, x1),
            ],
            '╡' => vec![v(cx, y0, y1), hz(ha, x0, cx + h), hz(hb, x0, cx + h)],
            '╢' => vec![v(va, y0, y1), v(vb, y0, y1), hz(cy, x0, va + h)],
            '╣' => vec![
                v(vb, y0, y1),
                v(va, y0, ha + h),
                v(va, hb - h, y1),
                hz(ha, x0, va + h),
                hz(hb, x0, va + h),
            ],
            '╤' => vec![hz(ha, x0, x1), hz(hb, x0, x1), v(cx, hb - h, y1)],
            '╥' => vec![hz(cy, x0, x1), v(va, cy - h, y1), v(vb, cy - h, y1)],
            '╦' => vec![
                hz(ha, x0, x1),
                hz(hb, x0, va + h),
                hz(hb, vb - h, x1),
                v(va, hb - h, y1),
                v(vb, hb - h, y1),
            ],
            '╧' => vec![hz(ha, x0, x1), hz(hb, x0, x1), v(cx, y0, ha + h)],
            '╨' => vec![hz(cy, x0, x1), v(va, y0, cy + h), v(vb, y0, cy + h)],
            '╩' => vec![
                hz(hb, x0, x1),
                hz(ha, x0, va + h),
                hz(ha, vb - h, x1),
                v(va, y0, ha + h),
                v(vb, y0, ha + h),
            ],
            '╪' => vec![v(cx, y0, y1), hz(ha, x0, x1), hz(hb, x0, x1)],
            '╫' => vec![v(va, y0, y1), v(vb, y0, y1), hz(cy, x0, x1)],
            '╬' => vec![
                v(va, y0, ha + h),
                v(vb, y0, ha + h),
                v(va, hb - h, y1),
                v(vb, hb - h, y1),
                hz(ha, x0, va + h),
                hz(ha, vb - h, x1),
                hz(hb, x0, va + h),
                hz(hb, vb - h, x1),
            ],
            _ => return None,
        })
    }

    /// 圆角：`╭╮╯╰`。两条直臂加一段四分之一圆环的路径，圆环与直臂重叠一个
    /// 物理像素（`lap`），接缝处才不会露白。
    fn rounded(&self, c: char) -> Option<Vec<Ink>> {
        let (sx, sy): (f32, f32) = match c {
            '╭' => (1., 1.),
            '╮' => (-1., 1.),
            '╯' => (-1., -1.),
            '╰' => (1., -1.),
            _ => return None,
        };
        let h = self.t / 2.;
        let r = ((self.x1 - self.x0).min(self.y1 - self.y0) / 2.).max(h * 2.);
        let (cx, cy) = (self.cx, self.cy);
        let mut ink = Vec::new();
        let lap = 1. / self.scale;
        if sy > 0. {
            ink.push(self.vstroke(cx, self.t, cy + r - lap, self.y1));
        } else {
            ink.push(self.vstroke(cx, self.t, self.y0, cy - r + lap));
        }
        if sx > 0. {
            ink.push(self.hstroke(cy, self.t, cx + r - lap, self.x1));
        } else {
            ink.push(self.hstroke(cy, self.t, self.x0, cx - r + lap));
        }
        let (ax, ay) = (cx + sx * r, cy + sy * r);
        let at = |radius: f32, theta: f32| {
            let (x, y) = (
                ax - sx * radius * theta.cos(),
                ay - sy * radius * theta.sin(),
            );
            point(px(x), px(y))
        };
        const SEGS: usize = 3;
        const INNER_PTS: usize = 4;
        let step = std::f32::consts::FRAC_PI_2 / SEGS as f32;
        let mut path: Option<gpui_kit::Path<Pixels>> = None;
        for i in 0..SEGS {
            let t0 = step * i as f32;
            let t1 = step * (i + 1) as f32;
            let start = at(r + h, t0);
            let p = match path.as_mut() {
                Some(p) => {
                    p.move_to(start);
                    p
                }
                None => path.insert(gpui_kit::Path::new(start)),
            };
            let ctrl = at((r + h) / (step / 2.).cos(), (t0 + t1) / 2.);
            p.curve_to(at(r + h, t1), ctrl);
            p.line_to(at(r - h, t1));
            for k in (0..INNER_PTS).rev() {
                p.line_to(at(r - h, t0 + (t1 - t0) * k as f32 / INNER_PTS as f32));
            }
        }
        if let Some(p) = path {
            ink.push(Ink::Path(p));
        }
        Some(ink)
    }

    /// 虚线：二段、三段、四段，粗细各两档。
    fn dashed(&self, c: char) -> Option<Vec<Ink>> {
        let (n, heavy, vertical) = match c {
            '╌' => (2, false, false),
            '╍' => (2, true, false),
            '╎' => (2, false, true),
            '╏' => (2, true, true),
            '┄' => (3, false, false),
            '┅' => (3, true, false),
            '┆' => (3, false, true),
            '┇' => (3, true, true),
            '┈' => (4, false, false),
            '┉' => (4, true, false),
            '┊' => (4, false, true),
            '┋' => (4, true, true),
            _ => return None,
        };
        let w = if heavy { self.t * 2. } else { self.t };
        let (a0, a1) = if vertical {
            (self.y0, self.y1)
        } else {
            (self.x0, self.x1)
        };
        let seg = (a1 - a0) / n as f32;
        let ink = (0..n)
            .map(|i| {
                let s = a0 + seg * (i as f32 + 0.15);
                let len = seg * 0.7;
                if vertical {
                    self.vstroke(self.cx, w, s, s + len)
                } else {
                    self.hstroke(self.cy, w, s, s + len)
                }
            })
            .collect();
        Some(ink)
    }

    /// 斜线 `╱╲╳`。斜着的笔画没法用矩形表示，走路径；`v` 是把水平厚度换算成
    /// 沿斜边量出来的厚度。
    fn diagonal(&self, c: char) -> Option<Vec<Ink>> {
        let (w, hgt) = (self.x1 - self.x0, self.y1 - self.y0);
        let v = self.t * (w * w + hgt * hgt).sqrt() / w;
        let p = |x: f32, y: f32| point(px(x), px(y));
        let quad = |top_x: f32, bot_x: f32| {
            let mut path = gpui_kit::Path::new(p(top_x, self.y0));
            path.line_to(p(top_x, self.y0 + v));
            path.line_to(p(bot_x, self.y1));
            path.line_to(p(bot_x, self.y1 - v));
            Ink::Path(path)
        };
        Some(match c {
            '╱' => vec![quad(self.x1, self.x0)],
            '╲' => vec![quad(self.x0, self.x1)],
            '╳' => vec![quad(self.x1, self.x0), quad(self.x0, self.x1)],
            _ => return None,
        })
    }

    /// 块元素：U+2580–U+259F。btop / htop 的柱状图就是这一段。
    fn blocks(&self, c: char) -> Option<Vec<Ink>> {
        let (x0, x1, y0, y1, cx, cy) = (self.x0, self.x1, self.y0, self.y1, self.cx, self.cy);
        let (w, hgt) = (x1 - x0, y1 - y0);
        let r = |x: f32, y: f32, ww: f32, hh: f32| self.rect(x, y, ww, hh);
        let ul = || r(x0, y0, cx - x0, cy - y0);
        let ur = || r(cx, y0, x1 - cx, cy - y0);
        let ll = || r(x0, cy, cx - x0, y1 - cy);
        let lr = || r(cx, cy, x1 - cx, y1 - cy);
        Some(match c {
            '▀' => vec![r(x0, y0, w, hgt / 2.)],
            '▁'..='█' => {
                let k = (c as u32 - 0x2580) as f32;
                let hh = hgt * k / 8.;
                vec![r(x0, y1 - hh, w, hh)]
            }
            '▉'..='▏' => {
                let k = (0x2590 - c as u32) as f32;
                vec![r(x0, y0, w * k / 8., hgt)]
            }
            '▐' => vec![r(cx, y0, x1 - cx, hgt)],
            '░' => vec![Ink::Shade(self.rectb(x0, y0, w, hgt), 0.25)],
            '▒' => vec![Ink::Shade(self.rectb(x0, y0, w, hgt), 0.5)],
            '▓' => vec![Ink::Shade(self.rectb(x0, y0, w, hgt), 0.75)],
            '▔' => vec![r(x0, y0, w, hgt / 8.)],
            '▕' => vec![r(x1 - w / 8., y0, w / 8., hgt)],
            '▖' => vec![ll()],
            '▗' => vec![lr()],
            '▘' => vec![ul()],
            '▙' => vec![ul(), ll(), lr()],
            '▚' => vec![ul(), lr()],
            '▛' => vec![ul(), ur(), ll()],
            '▜' => vec![ul(), ur(), lr()],
            '▝' => vec![ur()],
            '▞' => vec![ur(), ll()],
            '▟' => vec![ur(), ll(), lr()],
            _ => return None,
        })
    }
}

/// 十字类字符的四条臂各是什么粗细：(上, 下, 左, 右)。
///
/// 这张表没有"设计"的空间 —— Unicode 给每个码位定死了四个方向的粗细组合，
/// 照抄即可。
fn arms_of(c: char) -> Option<(Arm, Arm, Arm, Arm)> {
    use Arm::{Heavy as H, Light as L, None as N};
    Some(match c {
        '─' => (N, N, L, L),
        '━' => (N, N, H, H),
        '│' => (L, L, N, N),
        '┃' => (H, H, N, N),
        '┌' => (N, L, N, L),
        '┍' => (N, L, N, H),
        '┎' => (N, H, N, L),
        '┏' => (N, H, N, H),
        '┐' => (N, L, L, N),
        '┑' => (N, L, H, N),
        '┒' => (N, H, L, N),
        '┓' => (N, H, H, N),
        '└' => (L, N, N, L),
        '┕' => (L, N, N, H),
        '┖' => (H, N, N, L),
        '┗' => (H, N, N, H),
        '┘' => (L, N, L, N),
        '┙' => (L, N, H, N),
        '┚' => (H, N, L, N),
        '┛' => (H, N, H, N),
        '├' => (L, L, N, L),
        '┝' => (L, L, N, H),
        '┞' => (H, L, N, L),
        '┟' => (L, H, N, L),
        '┠' => (H, H, N, L),
        '┡' => (H, L, N, H),
        '┢' => (L, H, N, H),
        '┣' => (H, H, N, H),
        '┤' => (L, L, L, N),
        '┥' => (L, L, H, N),
        '┦' => (H, L, L, N),
        '┧' => (L, H, L, N),
        '┨' => (H, H, L, N),
        '┩' => (H, L, H, N),
        '┪' => (L, H, H, N),
        '┫' => (H, H, H, N),
        '┬' => (N, L, L, L),
        '┭' => (N, L, H, L),
        '┮' => (N, L, L, H),
        '┯' => (N, L, H, H),
        '┰' => (N, H, L, L),
        '┱' => (N, H, H, L),
        '┲' => (N, H, L, H),
        '┳' => (N, H, H, H),
        '┴' => (L, N, L, L),
        '┵' => (L, N, H, L),
        '┶' => (L, N, L, H),
        '┷' => (L, N, H, H),
        '┸' => (H, N, L, L),
        '┹' => (H, N, H, L),
        '┺' => (H, N, L, H),
        '┻' => (H, N, H, H),
        '┼' => (L, L, L, L),
        '┽' => (L, L, H, L),
        '┾' => (L, L, L, H),
        '┿' => (L, L, H, H),
        '╀' => (H, L, L, L),
        '╁' => (L, H, L, L),
        '╂' => (H, H, L, L),
        '╃' => (H, L, H, L),
        '╄' => (H, L, L, H),
        '╅' => (L, H, H, L),
        '╆' => (L, H, L, H),
        '╇' => (H, L, H, H),
        '╈' => (L, H, H, H),
        '╉' => (H, H, H, L),
        '╊' => (H, H, L, H),
        '╋' => (H, H, H, H),
        '╴' => (N, N, L, N),
        '╵' => (L, N, N, N),
        '╶' => (N, N, N, L),
        '╷' => (N, L, N, N),
        '╸' => (N, N, H, N),
        '╹' => (H, N, N, N),
        '╺' => (N, N, N, H),
        '╻' => (N, H, N, N),
        '╼' => (N, N, L, H),
        '╽' => (L, H, N, N),
        '╾' => (N, N, H, L),
        '╿' => (H, L, N, N),
        _ => return None,
    })
}

// ─── powerline ──────────────────────────────────────────────────────────────
//
// starship、oh-my-zsh 的 agnoster 一类主题用这几个字符做段与段之间的分隔。
// 它们是 Nerd Font 私用区里的字形，多数等宽字体没有；就算有，也一样受制于
// 上面那条"字形按 em 盒设计、格子按行距算"的错位，接缝处会露出底色。

#[derive(Clone, Copy, PartialEq, Debug)]
enum PowerlineShape {
    TriangleRight,
    TriangleLeft,
    HalfCircleRight,
    HalfCircleLeft,
    SlantLowerLeft,
    SlantLowerRight,
    SlantUpperLeft,
    SlantUpperRight,
}

impl PowerlineShape {
    fn of(c: char) -> Option<Self> {
        Some(match c {
            '\u{e0b0}' => Self::TriangleRight,
            '\u{e0b2}' => Self::TriangleLeft,
            '\u{e0b4}' => Self::HalfCircleRight,
            '\u{e0b6}' => Self::HalfCircleLeft,
            '\u{e0b8}' => Self::SlantLowerLeft,
            '\u{e0ba}' => Self::SlantLowerRight,
            '\u{e0bc}' => Self::SlantUpperLeft,
            '\u{e0be}' => Self::SlantUpperRight,
            _ => return None,
        })
    }

    /// 这个形状"填满"的那条竖边在格子的哪一侧。
    ///
    /// 刻意写成穷举的 `match`：以后加形状时漏掉一个，编译器会拦下来，而不是
    /// 悄悄把补边画到错误的一侧。
    fn closing_edge_x(self, bounds: Bounds<Pixels>) -> Pixels {
        match self {
            Self::TriangleRight
            | Self::HalfCircleRight
            | Self::SlantLowerLeft
            | Self::SlantUpperLeft => bounds.left(),
            Self::TriangleLeft
            | Self::HalfCircleLeft
            | Self::SlantLowerRight
            | Self::SlantUpperRight => bounds.right(),
        }
    }
}

/// powerline 分隔符：U+E0B0/B2/B4/B6/B8/BA/BC/BE。
pub(crate) fn powerline(
    c: char,
    bounds: Bounds<Pixels>,
    scale: f32,
    fg_alpha: f32,
) -> Option<Vec<Ink>> {
    let shape = PowerlineShape::of(c)?;
    let mut ink = Vec::with_capacity(2);
    if let Some(edge) = powerline_solid_edge(bounds, shape, scale, fg_alpha) {
        ink.push(Ink::Rect(edge));
    }
    ink.push(Ink::Path(powerline_path(bounds, shape)));
    Some(ink)
}

fn powerline_path(bounds: Bounds<Pixels>, shape: PowerlineShape) -> gpui_kit::Path<Pixels> {
    let (x0, y0) = (bounds.origin.x, bounds.origin.y);
    let (x1, y1) = (x0 + bounds.size.width, y0 + bounds.size.height);
    let ymid = y0 + bounds.size.height / 2.;

    let tri = |a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>| {
        let mut p = gpui_kit::Path::new(a);
        p.line_to(b);
        p.line_to(c);
        p
    };
    let half_circle = |anchor_x: Pixels, dir: f32| {
        const SEGS: usize = 12;

        let (rx, ry) = (bounds.size.width.as_f32(), bounds.size.height.as_f32() / 2.);

        let at = |scale: f32, theta: f32| {
            let x = anchor_x.as_f32() + dir * rx * scale * theta.cos();
            let y = ymid.as_f32() + ry * scale * theta.sin();
            point(
                px(x.clamp(x0.as_f32(), x1.as_f32())),
                px(y.clamp(y0.as_f32(), y1.as_f32())),
            )
        };

        let step = std::f32::consts::PI / SEGS as f32;
        let mut p = gpui_kit::Path::new(at(1., -std::f32::consts::FRAC_PI_2));
        for i in 0..SEGS {
            let t0 = -std::f32::consts::FRAC_PI_2 + step * i as f32;
            let t1 = t0 + step;
            let ctrl = at(1. / (step / 2.).cos(), (t0 + t1) / 2.);
            p.curve_to(at(1., t1), ctrl);
        }
        p
    };
    match shape {
        PowerlineShape::TriangleRight => tri(point(x0, y0), point(x1, ymid), point(x0, y1)),
        PowerlineShape::TriangleLeft => tri(point(x1, y0), point(x0, ymid), point(x1, y1)),
        PowerlineShape::SlantLowerLeft => tri(point(x0, y0), point(x1, y1), point(x0, y1)),
        PowerlineShape::SlantLowerRight => tri(point(x1, y0), point(x1, y1), point(x0, y1)),
        PowerlineShape::SlantUpperLeft => tri(point(x0, y0), point(x1, y0), point(x0, y1)),
        PowerlineShape::SlantUpperRight => tri(point(x0, y0), point(x1, y0), point(x1, y1)),
        PowerlineShape::HalfCircleRight => half_circle(x0, 1.),
        PowerlineShape::HalfCircleLeft => half_circle(x1, -1.),
    }
}

/// 填充路径会对落在物理像素之间的那条"闭合边"做抗锯齿，于是分隔符与它左右
/// 那一格的底色之间透出一条浅缝。这里把那一列被部分覆盖的像素用不透明的前景
/// 色整个盖掉；边本来就对齐时不需要这一笔。
///
/// 只有不透明的分隔符才这么补：半透明的（SGR 2 暗色）会让盖的这块和路径叠加
/// 到一起，那一列的 alpha 就超过字形本身，反而把相邻格子染上色。
fn powerline_solid_edge(
    bounds: Bounds<Pixels>,
    shape: PowerlineShape,
    scale_factor: f32,
    fg_alpha: f32,
) -> Option<Bounds<Pixels>> {
    if !fg_alpha.is_finite() || fg_alpha < 1. {
        return None;
    }
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0. {
        scale_factor
    } else {
        1.
    };
    let physical_edge = shape.closing_edge_x(bounds).as_f32() * scale_factor;
    let physical_left = physical_edge.floor();
    let physical_right = physical_edge.ceil();
    if physical_left == physical_right {
        return None;
    }

    Some(Bounds::from_corners(
        point(px(physical_left / scale_factor), bounds.top()),
        point(px(physical_right / scale_factor), bounds.bottom()),
    ))
}

#[cfg(test)]
#[path = "../../tests/ui/elements/boxdraw.rs"]
mod tests;
