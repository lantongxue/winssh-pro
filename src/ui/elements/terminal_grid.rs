//! 终端网格元素：把一份 [`Content`] 画成一屏字符。
//!
//! 移植自 zed 的 `crates/terminal_view/src/terminal_element.rs`，去掉了
//! 超链接、搜索高亮与内联模式，留下终端必须有的四层：
//! **底色 → 自绘字形 → 文字 → 光标**。
//!
//! 第二层是 [`crate::ui::elements::boxdraw`]：制表符、块元素与 powerline
//! 分隔符不交给字体，按格子的矩形自己画。理由见那个模块的头注释 ——
//! 一句话是字体的字形按 em 盒设计，而格子是"字号 × 行距"算出来的另一个
//! 尺寸，交给字体画，格与格之间就会露出缝（btop 的边框会变成虚线）。
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
//!
//! 输入法算半个例外：组字串（preedit）由外面交进来，元素只负责把它画在
//! 光标那一格上；输入处理器也是外面造的，元素只在 paint 里替它登记 ——
//! `Window::handle_input` 只能在 paint 里调，而候选窗该贴着哪一格，
//! 也只有取完快照的这里才知道。

use gpui_kit::{
    App, Bounds, CursorStyle, Element, ElementId, Entity, Font, FontFeatures, FontStyle,
    FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement,
    LayoutId, Pixels, Point, Rgba, ShapedLine, SharedString, StrikethroughStyle, Style, TextAlign,
    TextRun, UnderlineStyle, Window, fill, outline, point, px, relative, size,
};

use alacritty_terminal::{index::Direction, term::search::Match, vte::ansi::CursorShape};

use crate::{
    terminal::{
        Content, Emulator, GridCell, GridPoint, SearchState, TerminalBounds, is_default_background,
    },
    ui::{elements::boxdraw, theme::ansi::AnsiPalette},
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

/// 不带任何装饰的一种字形，只有颜色。
fn cell_key(color: Hsla) -> StyleKey {
    StyleKey {
        color,
        bold: false,
        italic: false,
        underline: false,
        undercurl: false,
        strike: false,
    }
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
    /// 这一段是纯 ASCII，后面还能再接（见 [`Run::accepts`]）。
    ascii: bool,
}

impl Run {
    fn new(at: GridPoint, character: char, key: StyleKey, ascii: bool) -> Self {
        Self {
            at,
            text: String::from(character),
            cells: 1,
            key,
            ascii,
        }
    }

    /// 这个格子能不能接在这一段后面：字形一致，而且位置正好是下一格。
    ///
    /// 只有 ASCII 能并段，别的字符（中文、emoji、带组合符的字母）一个一段。
    /// Windows 上一段文字的行高与基线取的是 DirectWrite 整行的行度量，段里只要
    /// 混进一个靠回退字体（雅黑之类）画的字，整段的基线就跟着回退字体走 —— 同一
    /// 行英文的上下位置于是取决于它和中文分没分在一段。而分段又随光标闪烁、
    /// 随删字在变，结果就是一闪一跳。宽字符还有一层：`force_width` 按"一个字形
    /// 一格"排，汉字占两格，和英文并在一段里位置就算错了。
    fn accepts(&self, at: GridPoint, key: &StyleKey, ascii: bool) -> bool {
        self.ascii
            && ascii
            && self.at.line == at.line
            && self.at.column + self.cells == at.column
            && &self.key == key
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

/// 一屏格子上"哪些是搜索命中"的查询表。
///
/// 命中在 [`crate::terminal::SearchState`] 里是若干个跨行的区间，而画格子是
/// 一格一问。每帧把区间摊平成一张按屏幕坐标索引的表，逐格 O(1) 查 ——
/// 反过来每格去遍历几千个命中区间，满屏命中时就是平方级的。
///
/// 没开搜索时整张表不分配。
#[derive(Default)]
struct MatchMap {
    /// 每个可见格子一个字节：0 = 不是命中，1 = 命中，2 = 当前那一个。
    flags: Vec<u8>,
    columns: usize,
}

impl MatchMap {
    const HIT: u8 = 1;
    const CURRENT: u8 = 2;

    /// 按快照的可见区域摊平。
    fn new(search: Option<&SearchState>, content: &Content) -> Self {
        let Some(search) = search else {
            return Self::default();
        };
        Self::build(
            search.matches(),
            search.current_index(),
            content.screen_lines,
            content.columns,
            content.display_offset as i32,
        )
    }

    /// 摊平那一步本身。和 [`Self::new`] 分开是为了能直接喂命中来测
    /// —— 跨行的区间、屏幕上下的截断、偏移换算，每一处都差得起一位。
    ///
    /// `offset` 是 `display_offset`：命中的行号是缓冲坐标，加上它才是屏幕行号
    /// —— 和 [`TerminalGridElement::layout_cells`] 里那一步是同一个换算。
    fn build(
        matches: &[Match],
        current: Option<usize>,
        rows: usize,
        columns: usize,
        offset: i32,
    ) -> Self {
        if rows == 0 || columns == 0 || matches.is_empty() {
            return Self::default();
        }

        let mut flags = vec![0u8; rows * columns];
        // 命中按网格顺序排着，所以整段在屏幕上方的是开头连续的一段，
        // 跳过它们；到了第一个落在屏幕下方的就可以收手。
        let skip = matches.partition_point(|m| m.end().line.0 + offset < 0);
        for (index, m) in matches.iter().enumerate().skip(skip) {
            let (start, end) = (*m.start(), *m.end());
            if start.line.0 + offset >= rows as i32 {
                break;
            }
            let value = match current == Some(index) {
                true => Self::CURRENT,
                false => Self::HIT,
            };
            for line in start.line.0..=end.line.0 {
                let row = line + offset;
                if row < 0 || row as usize >= rows {
                    continue;
                }
                // 跨行的命中：第一行从它自己的列起，中间几行整行，
                // 最后一行到它自己的列止。
                let first = match line == start.line.0 {
                    true => start.column.0,
                    false => 0,
                };
                let last = match line == end.line.0 {
                    true => end.column.0,
                    false => columns.saturating_sub(1),
                };
                let base = row as usize * columns;
                for column in first..=last.min(columns.saturating_sub(1)) {
                    flags[base + column] = value;
                }
            }
        }

        Self { flags, columns }
    }

    /// 这一格是不是命中。`at` 是屏幕坐标。
    fn at(&self, at: GridPoint) -> u8 {
        if self.flags.is_empty() || at.line < 0 || at.column >= self.columns {
            return 0;
        }
        let index = at.line as usize * self.columns + at.column;
        self.flags.get(index).copied().unwrap_or(0)
    }
}

/// 一个自己画的格子：制表符、块元素或 powerline 分隔符。`ink` 是它拆出来的
/// 几笔（矩形、半透明矩形、路径），坐标已经是最终的像素位置。
struct Native {
    ink: Vec<boxdraw::Ink>,
    color: Hsla,
}

/// prepaint 算好、paint 直接用的东西。
pub struct GridLayout {
    bounds: TerminalBounds,
    regions: Vec<Region>,
    natives: Vec<Native>,
    /// 每段文字和它的落笔位置（已经按主字体的基线对齐过）。
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<Cursor>,
    /// 光标所在的那一格（不管光标此刻亮不亮、是什么形状）。组字串画在这儿，
    /// 输入法候选窗也贴着它弹。光标滚出屏幕时为 `None`。
    caret: Option<Bounds<Pixels>>,
    /// 排好的组字串与它对齐基线要往下挪的量，没在组字时为 `None`。
    preedit: Option<(Pixels, ShapedLine)>,
    /// 组字串垫底的颜色：盖住它压着的那几格原有的字。
    preedit_background: Hsla,
    /// 指针样式挂靠的那块区域。
    hitbox: Hitbox,
    /// 对端此刻自己收鼠标。是的话就不设 I 形指针，让它落回平台默认的箭头
    /// —— 见 [`Element::paint`] 里那一句。
    mouse_reporting: bool,
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
    /// 光标这一帧画不画。闪烁的 off 阶段此值为 `false`，光标整个不画
    /// —— 和 `focused = false`（空心框）不是一回事。
    cursor_visible: bool,
    /// 输入法正在组的那串字（候选还没选定时的 `ni'hao`），空串表示没在组字。
    preedit: SharedString,
    /// paint 末尾调一次，参数是光标那一格：外面在里面登记输入处理器。
    on_input: Option<InputRegistration>,
}

/// 见 [`TerminalGridElement::on_input`]。
type InputRegistration = Box<dyn FnOnce(Option<Bounds<Pixels>>, &mut Window, &mut App)>;

impl TerminalGridElement {
    pub fn new(emulator: Entity<Emulator>, focused: bool, cursor_visible: bool) -> Self {
        Self {
            emulator,
            focused,
            cursor_visible,
            preedit: SharedString::default(),
            on_input: None,
        }
    }

    /// 把输入法的组字串画在光标那一格上。
    pub fn preedit(mut self, text: SharedString) -> Self {
        self.preedit = text;
        self
    }

    /// 在 paint 里登记输入处理器。
    ///
    /// 没有输入处理器，Windows 会把这扇窗的输入法上下文整个摘掉（gpui 的
    /// `update_ime_enabled`）—— 用户切到中文输入法也打不出中文，按键根本
    /// 到不了输入法那里。回调拿到的是光标那一格的矩形（窗口坐标），
    /// 候选窗靠它定位。
    pub fn on_input(
        mut self,
        register: impl FnOnce(Option<Bounds<Pixels>>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_input = Some(Box::new(register));
        self
    }

    /// 把快照里的格子切成"底色块"、"自绘格"与"文字段"三串。
    ///
    /// 三串分开是因为它们的绘制单位不一样：底色是矩形，能横跨若干格合成一块；
    /// 自绘格是按格子算出来的几笔，一格一份；文字要交给排版系统，一段里的
    /// 字形必须一致。
    ///
    /// `scale` 是 `window.scale_factor()`，只有自绘那一路用得着 —— 笔画要对齐
    /// 到物理像素，否则同一条竖线会一列 1px 一列 2px。
    fn layout_cells(
        content: &Content,
        palette: &AnsiPalette,
        bounds: &TerminalBounds,
        scale: f32,
        matches: &MatchMap,
        block_cursor: Option<GridPoint>,
    ) -> (Vec<Region>, Vec<Native>, Vec<Run>) {
        let mut regions: Vec<Region> = Vec::new();
        let mut natives: Vec<Native> = Vec::new();
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
            let (mut foreground, mut background) =
                cell_colors(cell, palette, selected, matches.at(at));

            // A focused block cursor is an opaque reverse-video cell. Paint its
            // background before glyphs and redraw the glyph in a contrasting
            // colour so the cursor never hides the character underneath it.
            let is_block_cursor = block_cursor.is_some_and(|cursor| {
                under_block_cursor(cell.point, cell.is_wide_char_spacer(), cursor)
            });
            if is_block_cursor {
                foreground = cursor_ink(palette.cursor(), palette.background(), foreground);
                background = Some(palette.cursor());
            }

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

            // 制表符、块元素、powerline 分隔符自己画，不走字体 —— 字体的字形
            // 按 em 盒设计，格子按"字号 × 行距"算，交给字体画就会在格与格之间
            // 露出缝。跳过之后这一段也就断在这里了：下一格的列号不再紧接着
            // `run.at.column + run.cells`，[`Run::accepts`] 自然不认。
            let mut character = cell.character();
            if let Some(ink) = boxdraw::ink(character, cell_rect(at, bounds), scale, foreground.a) {
                natives.push(Native {
                    ink,
                    color: foreground,
                });
                // 自绘只画字符本身。下划线、波浪线、删除线是 gpui 的 `TextRun`
                // 装饰，得有个字形挂着才画得出来，所以带这些标志的格子补一个
                // 空格进排版段；其余的到此为止。
                if !(cell.has_underline() || cell.has_strikeout()) {
                    continue;
                }
                character = ' ';
            }

            let key = cell_style(cell, foreground);
            // 带零宽字符的格子也自成一段：组合符多半要回退字体来画。
            let ascii = character.is_ascii() && zero_width.is_none();
            match &mut open {
                Some(run) if run.accepts(at, &key, ascii) => run.push(character, true),
                _ => {
                    runs.extend(open.take());
                    open = Some(Run::new(at, character, key, ascii));
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
        (regions, natives, runs)
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

        // 格子取字体给的自然尺寸，不拉伸去凑满容器。
        //
        // 以前这里把格子拉到 `available / cols`，为的是让网格严丝合缝贴住上下
        // 两条栏。代价是格宽不再等于字体的步进：`shape_line` 的 `force_width`
        // 只挪字形不缩字形，而且差值 ≤1px 时干脆不挪，于是每一格的字都带一点
        // 说不准的偏移。上下那条空隙其实不必这么治 —— 余量落在容器里，而容器
        // 的底色和终端的默认底色本来就是同一个（见 `TerminalPanel::render_terminal`
        // 的 `.bg(cx.theme().background)`），看不出来。
        //
        // 行高取整像素：整数行距让自绘的横线在每一行上厚度一致，也让上下相邻
        // 两格的竖线正好接上。这一条和 tty7 的 `TerminalElement::prepaint` 一致。
        let line_height = px(f32::from(line_height).round()).max(px(1.));
        let available_width = (bounds.size.width - PADDING_X * 2.).max(cell_width);
        let available_height = (bounds.size.height - PADDING_TOP - PADDING_BOTTOM).max(line_height);
        let cols = (available_width / cell_width).floor().max(1.) as usize;
        let rows = (available_height / line_height).floor().max(1.) as usize;
        let grid_origin = point(bounds.origin.x + PADDING_X, bounds.origin.y + PADDING_TOP);
        let grid_bounds = TerminalBounds::new(
            line_height,
            cell_width,
            Bounds {
                origin: grid_origin,
                // 网格真正占的那一块，不是可用区域：余量不算进去，
                // `num_columns()` / `num_lines()` 才和这里的 `cols` / `rows` 完全一致。
                size: size(cell_width * cols as f32, line_height * rows as f32),
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
        let scale = window.scale_factor();
        // 指针样式挂在这块地方上。用元素的整块 `bounds` 而不是 `grid_bounds`：
        // 左右那 8px 内边距也是终端的一部分，光标划过去不该跳一下。
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        // 读的是这一帧刚取的快照，不是上一帧的 —— 对端刚打开鼠标模式的那一帧
        // 指针就得变过来。挂在外层 `div` 上做不到这件事：`render` 跑在 prepaint
        // 之前，那时候看到的还是上一帧的模式。
        let mouse_reporting = self.emulator.read(cx).mouse_mode();
        // 借着读，不复制。一屏是几千个格子，每帧克隆一份纯属白费 ——
        // 排版只往前扫一遍，扫完就不再需要它了。
        let emulator = self.emulator.read(cx);
        let content = emulator.content();
        // 搜索命中摊平成按屏幕坐标查的一张表。没开搜索时它是空的，不分配。
        let matches = MatchMap::new(emulator.search(), content);
        let cursor = if self.focused && !self.cursor_visible {
            None
        } else {
            cursor(content, &grid_bounds, &palette, self.focused)
        };
        let block_cursor = if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.shape == CursorShape::Block)
        {
            content.cursor.map(|cursor| GridPoint {
                line: cursor.point.line,
                column: cursor.point.column,
            })
        } else {
            None
        };
        let (regions, natives, runs) = Self::layout_cells(
            content,
            &palette,
            &grid_bounds,
            scale,
            &matches,
            block_cursor,
        );
        let caret = caret_cell(content, &grid_bounds);
        // 组字串不按格子对齐（不传 `force_width`）：拼音是半角、汉字是全角，
        // 按单格宽硬排的话汉字会叠在一起。它只是临时浮在光标上的一层，
        // 不需要和下面的格子对得上。
        let preedit = (!self.preedit.is_empty() && caret.is_some()).then(|| {
            let key = StyleKey {
                underline: true,
                ..cell_key(palette.foreground())
            };
            let mut style = build_text_run(&key, &font);
            style.len = self.preedit.len();
            window.text_system().shape_line(
                self.preedit.clone(),
                font_size,
                std::slice::from_ref(&style),
                None,
            )
        });
        // 主字体的行度量。靠回退字体画的那些段（中文、emoji）行度量不一样，
        // gpui 按各段自己的度量摆基线，不对齐的话中文会比旁边的英文低一截。
        // 全是 ASCII 时用不着，不量。
        let main_metrics = (preedit.is_some() || runs.iter().any(|run| !run.ascii)).then(|| {
            let mut style = build_text_run(&cell_key(palette.foreground()), &font);
            style.len = 1;
            let line = window.text_system().shape_line(
                SharedString::new_static("m"),
                font_size,
                std::slice::from_ref(&style),
                None,
            );
            (line.ascent, line.descent)
        });
        let shift = |line: &ShapedLine| {
            main_metrics.map_or(px(0.), |main| {
                baseline_shift(line_height, main, (line.ascent, line.descent))
            })
        };
        let preedit = preedit.map(|line| (shift(&line), line));
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
                    // 逼着排版按格子对齐。格宽就是字体的自然步进，所以这一步
                    // 通常什么都不用挪 —— 它兜住的是字体里那些宽度不老实的字符。
                    Some(cell_width),
                );
                let mut origin = cell_origin(run.at, &grid_bounds);
                if !run.ascii {
                    origin.y += shift(&line);
                }
                (origin, line)
            })
            .collect();

        GridLayout {
            bounds: grid_bounds,
            regions,
            natives,
            lines,
            cursor,
            caret,
            preedit,
            preedit_background: palette.background(),
            hitbox,
            mouse_reporting,
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

        // 自绘的字形：制表符、块元素、powerline。坐标在 prepaint 里就按格子
        // 算好了，这里只管照着涂。
        for native in &layout.natives {
            for ink in &native.ink {
                match ink {
                    boxdraw::Ink::Rect(rect) => window.paint_quad(fill(*rect, native.color)),
                    // `░▒▓` 是同一个前景色按比例打薄，不是另一种颜色。
                    boxdraw::Ink::Shade(rect, alpha) => {
                        let mut color = native.color;
                        color.a *= alpha;
                        window.paint_quad(fill(*rect, color));
                    }
                    boxdraw::Ink::Path(path) => window.paint_path(path.clone(), native.color),
                }
            }
        }

        // 空心框画在字下面：它是个框，盖不住字，先画后画都一样，
        // 但放在这里就不必和实心块分两次判断。
        if let Some(cursor) = &layout.cursor
            && cursor.shape == CursorShape::HollowBlock
        {
            window.paint_quad(outline(
                cursor.bounds,
                cursor.color,
                gpui_kit::BorderStyle::Solid,
            ));
        }

        for (origin, line) in &layout.lines {
            // 排版失败（字体缺字形）就跳过这一段，别把整帧连坐。
            let _ = line.paint(
                *origin,
                bounds.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }

        if let Some(cursor) = &layout.cursor
            && cursor.shape != CursorShape::HollowBlock
            && cursor.shape != CursorShape::Block
        {
            window.paint_quad(fill(cursor.bounds, cursor.color));
        }

        // 组字串最后画，连光标一起盖住：组字的时候光标该在组字串里，
        // 而不是在它底下露出半截。
        if let (Some((shift, line)), Some(caret)) = (&layout.preedit, layout.caret) {
            let extent = size(line.width.ceil().max(bounds.cell_width), bounds.line_height);
            window.paint_quad(fill(
                Bounds::new(caret.origin, extent),
                layout.preedit_background,
            ));
            let _ = line.paint(
                point(caret.origin.x, caret.origin.y + *shift),
                bounds.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }

        if let Some(register) = self.on_input.take() {
            register(layout.caret, window, cx);
        }

        // 指针样式：能划词的时候是 I 形，对端自己收鼠标的时候**什么都不设**。
        //
        // 不设是关键 —— `set_cursor_style` 是每帧往这一帧的样式表里登记一条，
        // 不登记就没有人认领这块区域，`Window::reset_cursor_style` 那句
        // `unwrap_or(CursorStyle::Arrow)` 就把指针落回平台默认的箭头。这比
        // "设成 Arrow"更对：真正在做决定的是那些开了鼠标模式的程序（btop、
        // vim、lazygit），它们自己画选区，I 形指针会误导用户以为还能划词。
        //
        // 这里刻意不看 Shift，尽管按住 Shift 确实能抢回划词（见
        // `TerminalPanel::reports_mouse`）：指针跟着一个随时会松开的修饰键
        // 闪来闪去比不变更糟，各家终端也都是这么处理的。
        if !layout.mouse_reporting {
            window.set_cursor_style(CursorStyle::IBeam, &layout.hitbox);
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

/// 一个格子的矩形，**横向不取整**。自绘那一路专用。
///
/// [`cell_origin`] 会把 x 抹到整数逻辑像素，那对底色和文字都没关系（底色的
/// 宽度是向上取整的，本来就多盖一点），但自绘不行：格宽是小数时，
/// `floor(x₀ + n·w) + w` 和 `floor(x₀ + (n+1)·w)` 不是同一个数，横线在格与格
/// 之间会差出一像素 —— 正是自绘要消掉的那种缝。这里把没抹过的边界交给
/// [`boxdraw`]，由它统一 `snap` 到物理像素，第 n 格的右边界与第 n+1 格的
/// 左边界于是算的是同一个表达式，结果必然相等。
fn cell_rect(at: GridPoint, bounds: &TerminalBounds) -> Bounds<Pixels> {
    Bounds::new(
        point(
            bounds.bounds.origin.x + bounds.cell_width * at.column as f32,
            bounds.bounds.origin.y + bounds.line_height * at.line as f32,
        ),
        size(bounds.cell_width, bounds.line_height),
    )
}

/// 一个格子最终画成什么颜色：(前景, 底色)。底色是 `None` 表示"就是终端的
/// 底色"，不必单独画一块。
///
/// 底色的优先级：**当前命中 > 其余命中 > 选中 > 程序自己设的底色**。
/// 搜索压过选中，是因为搜索栏开着的时候用户看的就是那些黄块；而当前那一个
/// 又必须压过其余的，不然"我现在在第几个上"就没有着落了。选中与命中一样
/// 是半透明的洗色：叠在格子原有的底色上，而不是顶替它。
fn cell_colors(
    cell: &GridCell,
    palette: &AnsiPalette,
    selected: bool,
    match_flag: u8,
) -> (Hsla, Option<Hsla>) {
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

    let background = (!is_default_background(background)).then(|| palette.color(background));

    let tint = match match_flag {
        MatchMap::CURRENT => Some(palette.match_current()),
        MatchMap::HIT => Some(palette.match_hit()),
        _ => None,
    };
    if let Some(tint) = tint {
        // 高亮是半透明的，得**合成**到这一格原本的底色上再交出去，不能直接
        // 顶替。直接顶替的话，程序自己画的底色（`ls` 的目录、diff 的红绿、
        // 反显的提示符）会被换成"琥珀色压在终端默认底色上"，命中落在什么
        // 东西上就看不出来了。底色画的是一块不透明四边形，所以合成在这儿做，
        // 而不是叠两层四边形。
        return (
            foreground,
            Some(over(tint, background.unwrap_or(palette.background()))),
        );
    }

    if selected {
        // 选中是半透明的一层"洗色"，叠在格子原本的底色上 —— 和上面搜索命中
        // 同一条规矩：`ls` 的目录、diff 的红绿、反显的提示符在选中时还得看
        // 得出来，顶替成一块实底就把这些信息全洗掉了。
        return (
            foreground,
            Some(over(palette.selection(), background.unwrap_or(palette.background()))),
        );
    }

    (foreground, background)
}

/// Pick the glyph colour for an opaque block cursor. The terminal background
/// is the conventional choice; fall back to the normal foreground when the
/// caret is closer to the background than to the text colour.
fn cursor_ink(caret: Hsla, background: Hsla, foreground: Hsla) -> Hsla {
    fn luminance(color: Hsla) -> f32 {
        let rgba: Rgba = color.into();
        let linear = |channel: f32| {
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(rgba.r) + 0.7152 * linear(rgba.g) + 0.0722 * linear(rgba.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
        (lighter + 0.05) / (darker + 0.05)
    }

    if contrast(caret, background) >= contrast(caret, foreground) {
        background
    } else {
        foreground
    }
}

/// 把半透明的 `top` 合成到不透明的 `bottom` 上，得到一个不透明色。
fn over(top: Hsla, bottom: Hsla) -> Hsla {
    let (top, bottom) = (gpui_kit::Rgba::from(top), gpui_kit::Rgba::from(bottom));
    let a = top.a.clamp(0., 1.);
    gpui_kit::Rgba {
        r: top.r * a + bottom.r * (1. - a),
        g: top.g * a + bottom.g * (1. - a),
        b: top.b * a + bottom.b * (1. - a),
        a: 1.,
    }
    .into()
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
    let cell = caret_cell(content, bounds)?;
    let origin = cell.origin;

    let shape = match focused {
        true => cursor.shape,
        false => CursorShape::HollowBlock,
    };
    let extent = match shape {
        CursorShape::Beam => size(BAR_WIDTH, bounds.line_height),
        CursorShape::Underline => size(cell.size.width, UNDERLINE_THICKNESS * 2.),
        _ => cell.size,
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

/// 一段文字要往下挪多少，基线才和主字体那一段落在同一条线上。
///
/// gpui 画一段文字时把它的行度量（`ascent + descent`）在行高里上下居中，
/// 基线落在 `(行高 - ascent - descent) / 2 + ascent`。回退字体（雅黑之类）的
/// 度量比等宽字体高得多，单独成段时基线就和旁边的英文差出一截，这里补回来。
fn baseline_shift(line_height: Pixels, main: (Pixels, Pixels), run: (Pixels, Pixels)) -> Pixels {
    let baseline =
        |(ascent, descent): (Pixels, Pixels)| (line_height - ascent - descent) / 2. + ascent;
    baseline(main) - baseline(run)
}

/// 光标所在那一格的矩形，不看光标的形状与闪烁。
///
/// 快照里的光标行号是相对可见区域的，滚动之后要跟着挪；挪出屏幕就是 `None`。
/// 光标压在宽字符（CJK、emoji）上时是两格宽 —— alacritty 已经把落在后半格
/// 占位符上的光标挪回了前半格，这里只需要把它撑到整个字。
fn caret_cell(content: &Content, bounds: &TerminalBounds) -> Option<Bounds<Pixels>> {
    let cursor = content.cursor?;
    let line = cursor.point.line + content.display_offset as i32;
    if line < 0 || line >= content.screen_lines as i32 {
        return None;
    }
    let at = GridPoint {
        line,
        column: cursor.point.column,
    };
    // 快照的格子按屏幕行逐行排满，屏幕坐标直接换成下标。
    let wide = content
        .cells
        .get(line as usize * content.columns + at.column)
        .is_some_and(GridCell::is_wide_char);
    let cells = if wide { 2. } else { 1. };
    Some(Bounds::new(
        cell_origin(at, bounds),
        size(bounds.cell_width * cells, bounds.line_height),
    ))
}

/// 这一格是否落在实心块光标底下：光标那一格本身，或者光标压着的宽字符的
/// 后半格占位符（就在光标右边一格）。
///
/// `point` 与 `cursor` 都是缓冲坐标。
fn under_block_cursor(point: GridPoint, wide_char_spacer: bool, cursor: GridPoint) -> bool {
    point == cursor
        || (wide_char_spacer && point.line == cursor.line && point.column == cursor.column + 1)
}

/// 指针位置落在屏幕的第几行第几列，(列, 行)，都是 0 起。
///
/// 这是"看得见的那一屏"的坐标：往回滚之后第一行仍然是 0。鼠标上报要的就是
/// 这一套 —— 对端根本不知道我们留了回卷区。
pub fn cell_at(
    position: Point<Pixels>,
    bounds: &TerminalBounds,
    content: &Content,
) -> (usize, usize) {
    let origin = bounds.bounds.origin;
    let column = (position.x - origin.x) / bounds.cell_width;
    let line = (position.y - origin.y) / bounds.line_height;
    (
        (column.floor().max(0.) as usize).min(content.columns.saturating_sub(1)),
        (line.floor().max(0.) as usize).min(content.screen_lines.saturating_sub(1)),
    )
}

/// 指针位置落在哪个格子上、以及在那格的哪半边，供拖选用。半边说的是指针
/// 离这格的左沿近还是右沿近：alacritty 的选区锚点带一个"边"
/// （`Direction::Left` / `Right`），右半边意味着"锚在这格**之后**"——
/// 没有它，选区的两端永远差半格，松手时最后那个字总是差一点没选进来
/// （tty7 的 `pos_to_cell` 同款判定）。
///
/// 行号按快照的坐标系给（可见区域第一行是 0，回卷区是负数），所以调用方
/// 拿到的和 [`Content::cells`] 里的 `point` 是同一套坐标。和 [`cell_at`] 的
/// 区别就在这儿：那个是屏幕坐标，这个是缓冲坐标。
pub fn point_at(
    position: Point<Pixels>,
    bounds: &TerminalBounds,
    content: &Content,
) -> (GridPoint, Direction) {
    let origin = bounds.bounds.origin;
    // 不夹半格判定用的那个分数：指针在网格外面（左半边是负数、右半边超过 1）
    // 时，夹回边界的那一列该取哪半边，正好由原分数的符号与大小说出来。
    let column = (position.x - origin.x) / bounds.cell_width;
    let side = if column - column.floor() <= 0.5 {
        Direction::Left
    } else {
        Direction::Right
    };
    let (column, line) = cell_at(position, bounds, content);
    (
        GridPoint {
            // 夹的是"屏幕上第几行"，减完才是缓冲里的行号。
            line: line as i32 - content.display_offset as i32,
            column,
        },
        side,
    )
}

#[cfg(test)]
#[path = "../../tests/ui/elements/terminal_grid.rs"]
mod tests;
