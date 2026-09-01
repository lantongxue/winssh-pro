//! 终端网格元素：把一份 [`Content`] 画成一屏字符。
//!
//! 移植自 zed 的 `crates/terminal_view/src/terminal_element.rs`，去掉了
//! 超链接、搜索高亮、块状字符的次格子绘制与内联模式，留下终端必须有的三层：
//! **底色 → 文字 → 光标**。
//!
//! 为什么要自己写一个 `Element`，而不是拿 `div` 拼：
//!
//! - **尺寸反过来决定内容。** 终端有多少行多少列，是"这块地方能放下几个
//!   格子"算出来的，所以必须先拿到布局结果，再回头 resize 网格。这件事
//!   只有 [`Element::prepaint`] 那个位置做得了。
//! - **一屏是几千个格子。** 一格一个 `div` 的话，光是布局树就压死了。
//!   这里按"颜色和字形连续"把一行切成几段，一段调一次 `shape_line`
//!   —— 满屏文本通常几十段，不是几千个。
//!
//! 交互不在这里：按键、滚轮、拖选都挂在外面那个 `div` 上
//! （见 [`crate::ui::panels::TerminalPanel`]），元素只管画。
//! [`point_at`] 是它们唯一需要的换算：指针位置落在哪个格子上。

use std::time::Instant;

use gpui::{
    App, Bounds, Element, ElementId, Entity, Font, FontFeatures, FontStyle, FontWeight,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point, ShapedLine,
    StrikethroughStyle, Style, TextAlign, TextRun, UnderlineStyle, Window, fill, outline, point,
    px, relative, size,
};

use alacritty_terminal::vte::ansi::CursorShape;

use crate::{
    terminal::{
        Content, Emulator, GridCell, GridPoint, TerminalBounds, fps, is_default_background,
    },
    ui::theme::ansi::AnsiPalette,
};

/// 终端网格左右留空、上下贴边：上下 0 彻底消除与信息栏/状态栏的间距，
/// 左边留 8px 使首列文字不贴着侧边栏边框（截图红框），右侧同样留空保持对称。
/// 原四边 8px 会在上下各留出一条空隙，现仅保留水平方向。
const PADDING_X: Pixels = px(8.);
const PADDING_TOP: Pixels = px(0.);
const PADDING_BOTTOM: Pixels = px(0.);

/// 光标是竖条（`\x1b[5 q`）时它的粗细。
const BAR_WIDTH: Pixels = px(2.);

/// 下划线光标与下划线字形的粗细。
const UNDERLINE_THICKNESS: Pixels = px(1.);

/// 暗色（SGR 2）把前景压到几成。跟着 alacritty 的观感走。
const DIM_ALPHA: f32 = 0.7;

/// 决定一段文字长什么样的几个开关。每格都算一次，所以它必须是几个可以
/// 直接比相等的小字段 —— 不能是 `TextRun`：那玩意儿带 `Font`（内含 `Vec`
/// 的 fallback、`Arc`），每格 clone 一次是刷屏时除快照外最大的一笔拷贝。
/// 真正交给排版的 `TextRun` 只在"一段收尾"时按这个 key 现造一个。
///
/// 在同一个基础 `font` 下，这个 key 相等 ⟺ 造出来的 `TextRun` 相等，
/// 所以用它判断能不能并段，和用 `TextRun` 判断完全等价。
#[derive(PartialEq)]
struct StyleKey {
    color: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    undercurl: bool,
    strike: bool,
}

/// 一段连续的文字：同一行、同一套字形与颜色，紧挨着的若干格。
struct Run {
    /// 这一段第一个字符落在第几行第几列。
    at: GridPoint,
    text: String,
    /// 这一段占了几格。宽字符占两格、零宽字符不占格，
    /// 所以它和 `text` 的长度不是一回事。
    cells: usize,
    key: StyleKey,
}

impl Run {
    fn new(at: GridPoint, character: char, key: StyleKey) -> Self {
        Self {
            at,
            text: String::from(character),
            cells: 1,
            key,
        }
    }

    /// 这个格子能不能接在这一段后面：字形一致，而且位置正好是下一格。
    fn accepts(&self, at: GridPoint, key: &StyleKey) -> bool {
        self.at.line == at.line && self.at.column + self.cells == at.column && &self.key == key
    }

    fn push(&mut self, character: char, counts_cell: bool) {
        self.text.push(character);
        if counts_cell {
            self.cells += 1;
        }
    }
}

/// 一块单色的底：同一行里连续若干格同一个底色。
struct Region {
    at: GridPoint,
    cells: usize,
    color: Hsla,
}

/// prepaint 算好、paint 直接用的东西。
pub struct GridLayout {
    bounds: TerminalBounds,
    regions: Vec<Region>,
    lines: Vec<(GridPoint, ShapedLine)>,
    cursor: Option<Cursor>,
    /// 度量关掉时是 `None`，一次 `Instant::now()` 都不取（dev 默认开着，release
    /// 需 `WINSSH_FPS`）。开着时从 prepaint 记到 paint 末，量的就是这一帧网格的真实成本。
    painted_at: Option<Instant>,
}

/// 光标画成的那一块。
struct Cursor {
    bounds: Bounds<Pixels>,
    color: Hsla,
    shape: CursorShape,
}

pub struct TerminalGridElement {
    emulator: Entity<Emulator>,
    /// 有焦点时光标按终端要求的形状画，没有焦点时一律是空心框
    /// —— 那时候它说的不是"光标什么样"，而是"敲键盘不会进这里"。
    focused: bool,
}

impl TerminalGridElement {
    pub fn new(emulator: Entity<Emulator>, focused: bool) -> Self {
        Self { emulator, focused }
    }

    /// 把快照里的格子切成"底色块"与"文字段"两串。
    ///
    /// 两串分开是因为它们的绘制单位不一样：底色是矩形，能横跨若干格合成一块；
    /// 文字要交给排版系统，一段里的字形必须一致。
    fn layout_cells(content: &Content, palette: &AnsiPalette) -> (Vec<Region>, Vec<Run>) {
        let mut regions: Vec<Region> = Vec::new();
        let mut runs: Vec<Run> = Vec::new();
        // 攒在手里还没收尾的那一段。
        let mut open: Option<Run> = None;
        // 上一格挂了零宽字符（重音符、emoji 变体选择符）。alacritty 会在
        // 这种格子后面补一个空格占位，那个空格不能当成真的空格画出来。
        let mut trailing_zero_width = false;

        for cell in &content.cells {
            // 快照里的行号是缓冲坐标：可见区域贴着底部时它就是屏幕上的行号，
            // 往回滚 N 行之后整段变成 -N 起头。画之前要换算成屏幕行号，
            // 换算不做的话滚上去的内容会被画到网格上沿之外，看着就是一片空白。
            // 选中判定仍然用缓冲坐标 —— 那是 alacritty 记选区的坐标系。
            let selected = content
                .selection
                .is_some_and(|selection| selection.contains(cell.point));
            let at = GridPoint {
                line: cell.point.line + content.display_offset as i32,
                column: cell.point.column,
            };
            let (foreground, background) = cell_colors(cell, palette, selected);

            if let Some(background) = background {
                // 能接上就把上一块拉长，接不上另起一块 —— 块数越少画得越少。
                match regions.last_mut() {
                    Some(last)
                        if last.color == background
                            && last.at.line == at.line
                            && last.at.column + last.cells == at.column =>
                    {
                        last.cells += 1;
                    }
                    _ => regions.push(Region {
                        at,
                        cells: 1,
                        color: background,
                    }),
                }
            }

            // 宽字符的后半格只是占位，没有字符要画。
            if cell.is_wide_char_spacer() {
                continue;
            }

            let zero_width = cell.zero_width().filter(|chars| !chars.is_empty());
            let filler = trailing_zero_width && cell.character() == ' ';
            trailing_zero_width = zero_width.is_some();
            if filler || cell.is_blank() {
                continue;
            }

            let key = cell_style(cell, foreground);
            match &mut open {
                Some(run) if run.accepts(at, &key) => run.push(cell.character(), true),
                _ => {
                    runs.extend(open.take());
                    open = Some(Run::new(at, cell.character(), key));
                }
            }

            // 零宽字符跟着上一个字符一起排版，自己不占格子。
            if let (Some(run), Some(chars)) = (open.as_mut(), zero_width) {
                for character in chars {
                    run.push(*character, false);
                }
            }
        }

        runs.extend(open);
        (regions, runs)
    }
}

impl Element for TerminalGridElement {
    type RequestLayoutState = ();
    type PrepaintState = GridLayout;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // 铺满给它的那一块。有多少行多少列由 prepaint 量出来的尺寸决定，
        // 而不是反过来由内容撑开 —— 终端是"给我一块地方，我按格子铺"。
        let style = Style {
            size: size(relative(1.).into(), relative(1.).into()),
            ..Default::default()
        };
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let text_style = window.text_style();
        let rem_size = window.rem_size();
        let font_size = text_style.font_size.to_pixels(rem_size);
        let line_height = text_style.line_height_in_pixels(rem_size);
        let font = Font {
            // 关掉连字。终端里 `!=`、`->`、`>>=` 是两三个格子里的两三个字符，
            // 连成一个字形之后既占错了格子，选中和光标也就对不上位置了。
            features: FontFeatures::disable_ligatures(),
            ..text_style.font()
        };

        // 格子宽度取 `m` 的步进：等宽字体里每个字符都一样宽，量哪个都行，
        // 而 `m` 是排版里量宽度的老规矩。
        let font_id = cx.text_system().resolve_font(&font);
        let cell_width = cx
            .text_system()
            .advance(font_id, font_size, 'm')
            .map(|advance| advance.width)
            // 量不出来只可能是字体加载失败。退回按字号估一个（等宽字体的
            // 字宽大约是字号的六成），总比这一帧画不出来好。
            .unwrap_or(font_size * 0.6);

        // 可用区域扣除内边距后，行列数向下取整。直接用 `cell_width/line_height`
        // 会因为容器高度不是行高的整数倍而留下余量，余量留在顶部或底部都会
        // 形成一条与状态栏/信息栏之间的空隙（两张截图红框）。为彻底消除上下
        // 间距，这里将格子尺寸拉伸到恰好填满容器：`adjusted_* = available / cols/rows`，
        // 使 `cols*adjusted_cell_width == available_width` 且
        // `rows*adjusted_line_height == available_height`，网格与上下
        // 栏零间距对齐，拉伸量每格 <1px，视觉无感。
        let available_width = (bounds.size.width - PADDING_X * 2.).max(cell_width);
        let available_height = (bounds.size.height - PADDING_TOP - PADDING_BOTTOM).max(line_height);
        let cols = (available_width / cell_width).floor().max(1.) as usize;
        let rows = (available_height / line_height).floor().max(1.) as usize;
        let adjusted_cell_width = available_width / cols as f32;
        let adjusted_line_height = available_height / rows as f32;
        let grid_origin = point(bounds.origin.x + PADDING_X, bounds.origin.y + PADDING_TOP);
        let grid_bounds = TerminalBounds::new(
            adjusted_line_height,
            adjusted_cell_width,
            Bounds {
                origin: grid_origin,
                size: size(available_width, available_height),
            },
        );

        // 先按量出来的尺寸把网格调好，再取快照 —— 顺序反了的话这一帧画的
        // 是上一个尺寸的内容，拖动窗口时会看到字被裁掉一截。
        //
        // 快照以前挂在事件泵的 `refresh` 里，每来一批字节就深拷整屏一次；现在
        // 挪到这儿，一帧只取一次。`resize` 也刻意不发 `cx.notify()`，不然它和
        // prepaint 会互相触发，成了一个停不下来的循环。
        self.emulator.update(cx, |emulator, _| {
            emulator.resize(grid_bounds);
            emulator.snapshot();
        });

        let palette = AnsiPalette::new(cx);
        // 借着读，不复制。一屏是几千个格子，每帧克隆一份纯属白费 ——
        // 排版只往前扫一遍，扫完就不再需要它了。
        let content = self.emulator.read(cx).content();
        let (regions, runs) = Self::layout_cells(content, &palette);
        let cursor = cursor(content, &grid_bounds, &palette, self.focused);
        let lines = runs
            .into_iter()
            .map(|run| {
                let len = run.text.len();
                let mut style = build_text_run(&run.key, &font);
                // `TextRun.len` 是这段的字节数，排版靠它切字符串。
                style.len = len;
                let line = window.text_system().shape_line(
                    run.text.into(),
                    font_size,
                    std::slice::from_ref(&style),
                    // 逼着排版按格子对齐：用拉伸后的格宽，使文字与网格列严格对齐
                    Some(adjusted_cell_width),
                );
                (run.at, line)
            })
            .collect();

        GridLayout {
            bounds: grid_bounds,
            regions,
            lines,
            cursor,
            // 从这一帧 prepaint 起算，到 paint 收尾。中间是取快照、切段、
            // 排版、画四边形这一整摊本帧的实际开销。
            painted_at: fps::enabled().then(Instant::now),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let bounds = &layout.bounds;

        for region in &layout.regions {
            let origin = cell_origin(region.at, bounds);
            // 宽度向上取整：格子宽度基本都是小数，不取整的话相邻两块底色
            // 之间会漏出一条一像素的缝。
            let extent = size(
                (bounds.cell_width * region.cells as f32).ceil(),
                bounds.line_height,
            );
            window.paint_quad(fill(Bounds::new(origin, extent), region.color));
        }

        // 空心框画在字下面：它是个框，盖不住字，先画后画都一样，
        // 但放在这里就不必和实心块分两次判断。
        if let Some(cursor) = &layout.cursor
            && cursor.shape == CursorShape::HollowBlock
        {
            window.paint_quad(outline(
                cursor.bounds,
                cursor.color,
                gpui::BorderStyle::Solid,
            ));
        }

        for (at, line) in &layout.lines {
            // 排版失败（字体缺字形）就跳过这一段，别把整帧连坐。
            let _ = line.paint(
                cell_origin(*at, bounds),
                bounds.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }

        if let Some(cursor) = &layout.cursor
            && cursor.shape != CursorShape::HollowBlock
        {
            window.paint_quad(fill(cursor.bounds, cursor.color));
        }

        // 收尾计时。`painted_at` 只有开了 `WINSSH_FPS` 才是 `Some`，
        // 平时这一句是一次 `Option` 判空，几乎无成本。
        if let Some(start) = layout.painted_at {
            fps::record(start.elapsed());
        }
    }
}

impl IntoElement for TerminalGridElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// 一个格子左上角的像素位置。横向取整，理由同上面那条缝。
fn cell_origin(at: GridPoint, bounds: &TerminalBounds) -> Point<Pixels> {
    point(
        (bounds.bounds.origin.x + bounds.cell_width * at.column as f32).floor(),
        bounds.bounds.origin.y + bounds.line_height * at.line as f32,
    )
}

/// 一个格子最终画成什么颜色：(前景, 底色)。底色是 `None` 表示"就是终端的
/// 底色"，不必单独画一块。
fn cell_colors(cell: &GridCell, palette: &AnsiPalette, selected: bool) -> (Hsla, Option<Hsla>) {
    let mut foreground = cell.foreground();
    let mut background = cell.background();
    // 反显（SGR 7）就是把两个颜色换过来。
    if cell.is_inverse() {
        std::mem::swap(&mut foreground, &mut background);
    }

    let mut foreground = palette.color(foreground);
    if cell.is_dim() {
        foreground.a *= DIM_ALPHA;
    }

    if selected {
        return (foreground, Some(palette.selection()));
    }

    let background = (!is_default_background(background)).then(|| palette.color(background));
    (foreground, background)
}

/// 一个格子的字形开关：粗、斜、下划线、删除线。每格算一次，只读几个 bit，
/// 不碰字体、不建 `TextRun`。
fn cell_style(cell: &GridCell, foreground: Hsla) -> StyleKey {
    StyleKey {
        color: foreground,
        bold: cell.is_bold(),
        italic: cell.is_italic(),
        underline: cell.has_underline(),
        undercurl: cell.has_undercurl(),
        strike: cell.has_strikeout(),
    }
}

/// 把一段的 `StyleKey` 落成排版要的 `TextRun`。一帧里每段各造一次，
/// 而不是每格一次 —— 这里是那个昂贵的 `font.clone()` 唯一发生的地方。
fn build_text_run(key: &StyleKey, font: &Font) -> TextRun {
    TextRun {
        len: 0,
        color: key.color,
        // 底色不走排版：一行里连续同色的格子已经合成一块矩形画掉了。
        background_color: None,
        font: Font {
            weight: match key.bold {
                true => FontWeight::BOLD,
                false => font.weight,
            },
            style: match key.italic {
                true => FontStyle::Italic,
                false => FontStyle::Normal,
            },
            ..font.clone()
        },
        underline: key.underline.then(|| UnderlineStyle {
            color: Some(key.color),
            thickness: UNDERLINE_THICKNESS,
            wavy: key.undercurl,
        }),
        strikethrough: key.strike.then_some(StrikethroughStyle {
            color: Some(key.color),
            thickness: UNDERLINE_THICKNESS,
        }),
    }
}

/// 光标那一块。滚上去看历史时不画 —— 它标的是"下一个字符落在哪儿"，
/// 而那个位置此刻不在屏幕上。
fn cursor(
    content: &Content,
    bounds: &TerminalBounds,
    palette: &AnsiPalette,
    focused: bool,
) -> Option<Cursor> {
    let cursor = content.cursor?;
    // 快照里的光标行号是相对可见区域的，滚动之后要跟着挪；挪出屏幕就不画。
    let line = cursor.point.line + content.display_offset as i32;
    if line < 0 || line >= content.screen_lines as i32 {
        return None;
    }

    let shape = match focused {
        true => cursor.shape,
        false => CursorShape::HollowBlock,
    };
    let origin = cell_origin(
        GridPoint {
            line,
            column: cursor.point.column,
        },
        bounds,
    );
    let extent = match shape {
        CursorShape::Beam => size(BAR_WIDTH, bounds.line_height),
        CursorShape::Underline => size(bounds.cell_width, UNDERLINE_THICKNESS * 2.),
        _ => size(bounds.cell_width, bounds.line_height),
    };
    // 下划线光标贴着格子底边，其余几种从左上角起。
    let origin = match shape {
        CursorShape::Underline => point(origin.x, origin.y + bounds.line_height - extent.height),
        _ => origin,
    };

    Some(Cursor {
        bounds: Bounds::new(origin, extent),
        color: palette.cursor(),
        shape,
    })
}

/// 指针位置落在哪个格子上，供拖选用。
///
/// 行号按快照的坐标系给（可见区域第一行是 0，回卷区是负数），所以调用方
/// 拿到的和 [`Content::cells`] 里的 `point` 是同一套坐标。
pub fn point_at(position: Point<Pixels>, bounds: &TerminalBounds, content: &Content) -> GridPoint {
    let origin = bounds.bounds.origin;
    let column = (position.x - origin.x) / bounds.cell_width;
    let line = (position.y - origin.y) / bounds.line_height;
    // 先夹在可见区域里再减偏移：夹的是"屏幕上第几行"，减完才是缓冲里的行号。
    let line = (line.floor() as i32).clamp(0, content.screen_lines.saturating_sub(1) as i32);

    GridPoint {
        line: line - content.display_offset as i32,
        column: (column.floor().max(0.) as usize).min(content.columns.saturating_sub(1)),
    }
}
