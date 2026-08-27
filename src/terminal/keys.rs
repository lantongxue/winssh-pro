//! 按键 → 终端字节序列。
//!
//! 照抄 zed 的 `crates/terminal/src/mappings/keys.rs`（它又是照着 alacritty 的
//! 键位表写的）。这张表没有“设计”的空间：xterm 定下来是什么样，终端里的程序
//! 就按什么样解析，改一个字节就是另一个键。
//!
//! 返回 `None` 表示这一下不该由这里翻译 —— 普通字符走 GPUI 的输入法通道
//! （`Keystroke::key_char`），由 [`super::Emulator::key_down`] 直接送字符。

use std::borrow::Cow;

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

/// 修饰键的组合。表里绝大多数条目只关心这五种情况，
/// 其余组合（带 Command / 三键同按）一律落到 [`Modifiers::Other`]。
#[derive(Debug, PartialEq, Eq)]
enum Modifiers {
    None,
    Alt,
    Ctrl,
    Shift,
    CtrlShift,
    Other,
}

impl Modifiers {
    fn new(keystroke: &Keystroke) -> Self {
        let modifiers = &keystroke.modifiers;
        match (
            modifiers.alt,
            modifiers.control,
            modifiers.shift,
            modifiers.platform,
        ) {
            (false, false, false, false) => Self::None,
            (true, false, false, false) => Self::Alt,
            (false, true, false, false) => Self::Ctrl,
            (false, false, true, false) => Self::Shift,
            (false, true, true, false) => Self::CtrlShift,
            _ => Self::Other,
        }
    }

    fn any(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// 一下按键对应的字节序列。`option_as_meta` 决定 macOS 上的 Option 是
/// 输入法用的组字键（默认）还是 Meta 前缀。
pub fn to_esc_str(
    keystroke: &Keystroke,
    mode: TermMode,
    option_as_meta: bool,
) -> Option<Cow<'static, str>> {
    let modifiers = Modifiers::new(keystroke);
    let app_cursor = mode.contains(TermMode::APP_CURSOR);

    // 固定绑定：键名 + 修饰键直接查出一个常量序列。
    let fixed: Option<&'static str> = match (keystroke.key.as_ref(), &modifiers) {
        // 基本控制键
        ("tab", Modifiers::None) => Some("\x09"),
        ("escape", Modifiers::None) => Some("\x1b"),
        ("enter", Modifiers::None) => Some("\x0d"),
        ("enter", Modifiers::Shift) => Some("\x0a"),
        ("enter", Modifiers::Alt) => Some("\x1b\x0d"),
        ("backspace", Modifiers::None) => Some("\x7f"),
        ("tab", Modifiers::Shift) => Some("\x1b[Z"),
        ("backspace", Modifiers::Ctrl) => Some("\x08"),
        ("backspace", Modifiers::Alt) => Some("\x1b\x7f"),
        ("backspace", Modifiers::Shift) => Some("\x7f"),
        ("space", Modifiers::Ctrl) => Some("\x00"),
        // 光标键与编辑键。应用光标模式下前缀是 SS3 而不是 CSI，
        // 这样 readline 之类的程序才分得清“方向键”和“Esc [ A”这串字符。
        ("home", Modifiers::None) if app_cursor => Some("\x1bOH"),
        ("home", Modifiers::None) => Some("\x1b[H"),
        ("end", Modifiers::None) if app_cursor => Some("\x1bOF"),
        ("end", Modifiers::None) => Some("\x1b[F"),
        ("up", Modifiers::None) if app_cursor => Some("\x1bOA"),
        ("up", Modifiers::None) => Some("\x1b[A"),
        ("down", Modifiers::None) if app_cursor => Some("\x1bOB"),
        ("down", Modifiers::None) => Some("\x1b[B"),
        ("right", Modifiers::None) if app_cursor => Some("\x1bOC"),
        ("right", Modifiers::None) => Some("\x1b[C"),
        ("left", Modifiers::None) if app_cursor => Some("\x1bOD"),
        ("left", Modifiers::None) => Some("\x1b[D"),
        ("insert", Modifiers::None) => Some("\x1b[2~"),
        ("delete", Modifiers::None) => Some("\x1b[3~"),
        ("pageup", Modifiers::None) => Some("\x1b[5~"),
        ("pagedown", Modifiers::None) => Some("\x1b[6~"),
        // 功能键
        ("f1", Modifiers::None) => Some("\x1bOP"),
        ("f2", Modifiers::None) => Some("\x1bOQ"),
        ("f3", Modifiers::None) => Some("\x1bOR"),
        ("f4", Modifiers::None) => Some("\x1bOS"),
        ("f5", Modifiers::None) => Some("\x1b[15~"),
        ("f6", Modifiers::None) => Some("\x1b[17~"),
        ("f7", Modifiers::None) => Some("\x1b[18~"),
        ("f8", Modifiers::None) => Some("\x1b[19~"),
        ("f9", Modifiers::None) => Some("\x1b[20~"),
        ("f10", Modifiers::None) => Some("\x1b[21~"),
        ("f11", Modifiers::None) => Some("\x1b[23~"),
        ("f12", Modifiers::None) => Some("\x1b[24~"),
        // 控制字符（caret notation）：Ctrl-A 是 0x01，一路到 Ctrl-Z 是 0x1a。
        // 大写那一行是 Ctrl-Shift 同按时 GPUI 报上来的键名。
        ("a", Modifiers::Ctrl) | ("A", Modifiers::CtrlShift) => Some("\x01"),
        ("b", Modifiers::Ctrl) | ("B", Modifiers::CtrlShift) => Some("\x02"),
        ("c", Modifiers::Ctrl) | ("C", Modifiers::CtrlShift) => Some("\x03"),
        ("d", Modifiers::Ctrl) | ("D", Modifiers::CtrlShift) => Some("\x04"),
        ("e", Modifiers::Ctrl) | ("E", Modifiers::CtrlShift) => Some("\x05"),
        ("f", Modifiers::Ctrl) | ("F", Modifiers::CtrlShift) => Some("\x06"),
        ("g", Modifiers::Ctrl) | ("G", Modifiers::CtrlShift) => Some("\x07"),
        ("h", Modifiers::Ctrl) | ("H", Modifiers::CtrlShift) => Some("\x08"),
        ("i", Modifiers::Ctrl) | ("I", Modifiers::CtrlShift) => Some("\x09"),
        ("j", Modifiers::Ctrl) | ("J", Modifiers::CtrlShift) => Some("\x0a"),
        ("k", Modifiers::Ctrl) | ("K", Modifiers::CtrlShift) => Some("\x0b"),
        ("l", Modifiers::Ctrl) | ("L", Modifiers::CtrlShift) => Some("\x0c"),
        ("m", Modifiers::Ctrl) | ("M", Modifiers::CtrlShift) => Some("\x0d"),
        ("n", Modifiers::Ctrl) | ("N", Modifiers::CtrlShift) => Some("\x0e"),
        ("o", Modifiers::Ctrl) | ("O", Modifiers::CtrlShift) => Some("\x0f"),
        ("p", Modifiers::Ctrl) | ("P", Modifiers::CtrlShift) => Some("\x10"),
        ("q", Modifiers::Ctrl) | ("Q", Modifiers::CtrlShift) => Some("\x11"),
        ("r", Modifiers::Ctrl) | ("R", Modifiers::CtrlShift) => Some("\x12"),
        ("s", Modifiers::Ctrl) | ("S", Modifiers::CtrlShift) => Some("\x13"),
        ("t", Modifiers::Ctrl) | ("T", Modifiers::CtrlShift) => Some("\x14"),
        ("u", Modifiers::Ctrl) | ("U", Modifiers::CtrlShift) => Some("\x15"),
        ("v", Modifiers::Ctrl) | ("V", Modifiers::CtrlShift) => Some("\x16"),
        ("w", Modifiers::Ctrl) | ("W", Modifiers::CtrlShift) => Some("\x17"),
        ("x", Modifiers::Ctrl) | ("X", Modifiers::CtrlShift) => Some("\x18"),
        ("y", Modifiers::Ctrl) | ("Y", Modifiers::CtrlShift) => Some("\x19"),
        ("z", Modifiers::Ctrl) | ("Z", Modifiers::CtrlShift) => Some("\x1a"),
        ("@", Modifiers::Ctrl) => Some("\x00"),
        ("[", Modifiers::Ctrl) => Some("\x1b"),
        ("\\", Modifiers::Ctrl) => Some("\x1c"),
        ("]", Modifiers::Ctrl) => Some("\x1d"),
        ("^", Modifiers::Ctrl) => Some("\x1e"),
        ("_", Modifiers::Ctrl) => Some("\x1f"),
        ("?", Modifiers::Ctrl) => Some("\x7f"),
        _ => None,
    };
    if let Some(esc) = fixed {
        return Some(Cow::Borrowed(esc));
    }

    // 带修饰键的光标键与功能键：序列里插一个修饰键编码。
    if modifiers.any() {
        let code = modifier_code(keystroke);
        let modified = match keystroke.key.as_ref() {
            "up" => Some(format!("\x1b[1;{code}A")),
            "down" => Some(format!("\x1b[1;{code}B")),
            "right" => Some(format!("\x1b[1;{code}C")),
            "left" => Some(format!("\x1b[1;{code}D")),
            "home" => Some(format!("\x1b[1;{code}H")),
            "end" => Some(format!("\x1b[1;{code}F")),
            "insert" => Some(format!("\x1b[2;{code}~")),
            "delete" => Some(format!("\x1b[3;{code}~")),
            "pageup" => Some(format!("\x1b[5;{code}~")),
            "pagedown" => Some(format!("\x1b[6;{code}~")),
            "f1" => Some(format!("\x1b[1;{code}P")),
            "f2" => Some(format!("\x1b[1;{code}Q")),
            "f3" => Some(format!("\x1b[1;{code}R")),
            "f4" => Some(format!("\x1b[1;{code}S")),
            "f5" => Some(format!("\x1b[15;{code}~")),
            "f6" => Some(format!("\x1b[17;{code}~")),
            "f7" => Some(format!("\x1b[18;{code}~")),
            "f8" => Some(format!("\x1b[19;{code}~")),
            "f9" => Some(format!("\x1b[20;{code}~")),
            "f10" => Some(format!("\x1b[21;{code}~")),
            "f11" => Some(format!("\x1b[23;{code}~")),
            "f12" => Some(format!("\x1b[24;{code}~")),
            _ => None,
        };
        if let Some(esc) = modified {
            return Some(Cow::Owned(esc));
        }
    }

    // Alt + 单个 ASCII 字符 = Esc 前缀。macOS 上 Option 默认是组字键
    // （Option-e 是重音符，不是 Meta-e），所以要显式打开才走这条路。
    if (!cfg!(target_os = "macos") || option_as_meta)
        && keystroke.modifiers.alt
        && keystroke.key.is_ascii()
        && keystroke.key.len() == 1
    {
        let key = keystroke.key.chars().next()?;
        if modifiers == Modifiers::Alt {
            return Some(Cow::Owned(format!("\x1b{key}")));
        }
        if keystroke.modifiers.shift {
            return Some(Cow::Owned(format!("\x1b{}", key.to_ascii_uppercase())));
        }
        if keystroke.modifiers.control && key.is_ascii_lowercase() {
            return Some(Cow::Owned(format!(
                "\x1b{}",
                (key as u8 - b'a' + 1) as char
            )));
        }
    }

    None
}

/// xterm 的修饰键编码：Shift = 1、Alt = 2、Ctrl = 4，按位或之后加一。
///
/// 见 <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-PC-Style-Function-Keys>。
fn modifier_code(keystroke: &Keystroke) -> u32 {
    let modifiers = &keystroke.modifiers;
    let mut code = 0;
    if modifiers.shift {
        code |= 1;
    }
    if modifiers.alt {
        code |= 1 << 1;
    }
    if modifiers.control {
        code |= 1 << 2;
    }
    code + 1
}
