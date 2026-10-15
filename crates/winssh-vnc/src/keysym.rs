//! GPUI 按键 → X11 keysym。
//!
//! RFB 的 `KeyEvent` 带的是一个 X11 keysym —— 那套 1985 年的编号：ASCII 段
//! 就是字符本身，0x100 以上是控制键，再往上（`0x01000000 | 码点`）是"这个
//! Unicode 字符"的兜底。GPUI 递给我们的是 `Keystroke`：`key` 是键帽上的名字
//! （小写），`key_char` 是这个组合真正打出来的字符。翻译按这个顺序：
//!
//! 1. 具名键查表（方向键、回车、功能键、编辑键 —— 这些没有"打出来的字符"）；
//! 2. **在打字**（没按住 ctrl / alt / cmd / fn）就按 `key_char` 走 Unicode
//!    规则：Latin-1 段 keysym 就是码点，其余加 `0x01000000` 前缀。`é`、`中`、
//!    `€` 都到得了对端；
//! 3. **在按组合键**就按键帽 `key` 走（shift 按着的字母取大写）。
//!
//! 第 2 步和第 3 步分开是这个模块的要害。`key_char` 的定义是"这个组合真正
//! 打出来的字符"，而组合键根本不打字：macOS 在 ctrl / cmd / fn 按着时一律
//! 不给 `key_char`（`gpui` 的 `parse_keystroke` 里那句
//! `if !control && !command && !function`），只按它发就是 ⌃C、⌘S 这些一个
//! 都发不出去；alt 那档更绕 —— ⌥S 在美式键盘上打出来的是 `ß`，照着发过去
//! 对端收到的是 Alt+ß，而人按的是 Alt+S。组合键要的是键帽，不是键帽在本地
//! 键盘布局下打出来的那个字符。
//!
//! 例外是 Windows 的 AltGr：它在事件里报成 ctrl+alt，但按的人是在打字
//! （AltGr+E 打 `€`）。这一下由 GPUI 的
//! [`KeyDownEvent::prefer_character_input`](gpui_kit::KeyDownEvent) 点名，
//! 顺着参数传进 [`keysym`]，照第 2 步走。
//!
//! 修饰键要单独说一句：RFB 没有"按下时 shift 已生效"这种概念，修饰键本身
//! 也是 keysym（`Shift_L`、`Control_L`……），要像普通键一样按下抬起各发一条。
//! GPUI 却只在 `Keystroke.modifiers` 里给状态不给过程，所以 [`modifier_keysyms`]
//! 把状态摊成"此刻哪些修饰键该按着"，由 [`super::keyboard`] 对着上一次的集合
//! 发差值。

use gpui_kit::{Keystroke, Modifiers};

/// X11 keysym 表里的修饰键。左右手各一颗，一律报左手那颗（`_L`）——
/// 对端只关心哪个修饰生效了，不关心是左右哪边的键帽。
pub const SHIFT: u32 = 0xffe1;
pub const CONTROL: u32 = 0xffe3;
pub const ALT: u32 = 0xffe9;
pub const SUPER: u32 = 0xffeb;

/// 这个 keysym 是不是修饰键。修饰键的按下抬起由 `modifier_keysyms` 的差值
/// 统一管，键帽自己到场的 `KeyEvent` 就不再重复发一条。
pub fn is_modifier(keysym: u32) -> bool {
    matches!(keysym, SHIFT | CONTROL | ALT | SUPER)
}

/// 此刻按着的修饰键（GPUI 的状态 → keysym 集合）。
pub fn modifier_keysyms(modifiers: &Modifiers) -> Vec<u32> {
    let mut keysyms = Vec::with_capacity(4);
    if modifiers.shift {
        keysyms.push(SHIFT);
    }
    if modifiers.control {
        keysyms.push(CONTROL);
    }
    if modifiers.alt {
        keysyms.push(ALT);
    }
    // GPUI 的 platform 键在 macOS 是 ⌘，在 Windows/Linux 是 Win / Super ——
    // 对 X11 来说那颗本来就是 Super_L。
    if modifiers.platform {
        keysyms.push(SUPER);
    }
    keysyms
}

/// 具名键表。键是 `Keystroke.key`（GPUI 统一转小写；mac 的 option 报 `alt`
/// 还是 `option` 各平台不一律，两个都收）。
fn named_keysym(key: &str) -> Option<u32> {
    let keysym = match key {
        "space" => 0x20,
        "tab" => 0xff09,
        "return" | "enter" => 0xff0d,
        "backspace" => 0xff08,
        "delete" => 0xffff,
        "escape" | "esc" => 0xff1b,
        "insert" => 0xff63,
        "home" => 0xff50,
        "end" => 0xff57,
        "pageup" | "prior" => 0xff55,
        "pagedown" | "next" => 0xff56,
        "left" => 0xff51,
        "up" => 0xff52,
        "right" => 0xff53,
        "down" => 0xff54,
        "capslock" => 0xffe5,
        "numlock" => 0xff7f,
        "scrolllock" => 0xff14,
        "printscreen" => 0xff61,
        "pause" => 0xff13,
        "menu" | "contextmenu" => 0xff67,
        "shift" => SHIFT,
        "control" | "ctrl" => CONTROL,
        "alt" | "option" => ALT,
        "cmd" | "command" | "platform" | "super" | "win" => SUPER,
        _ => return named_function_key(key),
    };
    Some(keysym)
}

/// `f1`–`f24`：keysym 连号，从 0xffbe 起。
fn named_function_key(key: &str) -> Option<u32> {
    let number = key.strip_prefix('f')?.parse::<u32>().ok()?;
    if (1..=24).contains(&number) {
        Some(0xffbe + number - 1)
    } else {
        None
    }
}

/// 这次击键该报给对端的 keysym。见模块文档的三步顺序。
///
/// `typing` 是"这一下该按它打出来的字符算"：平时就是"没按住 ctrl / alt /
/// cmd / fn"，Windows 的 AltGr（事件里报成 ctrl+alt，人却是在打字）由调用方
/// 按 `KeyDownEvent::prefer_character_input` 额外点名。
pub fn keysym(keystroke: &Keystroke, typing: bool) -> Option<u32> {
    if let Some(keysym) = named_keysym(&keystroke.key) {
        return Some(keysym);
    }
    // key_char 是"这个组合真正打出来的字符"：shift-a 给 `A`（keysym 0x41），
    // shift-2 在美式键盘给 `@`。它是 Option<String> 而不止一个字符是合理的
    //（IME 一次落一串）—— 取第一个码点就是我们要的。
    let typed = keystroke
        .key_char
        .as_deref()
        .and_then(|text| text.chars().next());
    if (typing || !is_chord(&keystroke.modifiers))
        && let Some(character) = typed
    {
        return Some(character_keysym(character));
    }
    // 组合键（以及 macOS 上压根没给 key_char 的那些）按键帽走。键帽只有一个
    // 字符才认：`f99`、`numpadclear` 这种表里没有的多字名字宁可不发 ——
    // 猜一个键发过去比不发更容易坏事。
    let mut characters = keystroke.key.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    // shift 按着时键帽上的字母该报大写：对端收到的是 `Ctrl+Shift+A`，
    // 而不是"按着 shift 的小写 a"。
    Some(character_keysym(match keystroke.modifiers.shift {
        true => character.to_ascii_uppercase(),
        false => character,
    }))
}

/// 一个字符的 keysym。Latin-1 及以下：keysym 就是码点。以上：
/// `0x01000000 | 码点`，X11 规定的 Unicode 段（RFC 6143 §7.5.4 直接引用它）。
fn character_keysym(character: char) -> u32 {
    let codepoint = u32::from(character);
    if codepoint < 0x100 {
        codepoint
    } else {
        0x0100_0000 | codepoint
    }
}

/// 这一下是组合键而不是在打字吗。shift 不算 —— 它本来就是打字的一部分
///（`shift-a` 打的是 `A`）。
fn is_chord(modifiers: &Modifiers) -> bool {
    modifiers.control || modifiers.alt || modifiers.platform || modifiers.function
}

#[cfg(test)]
#[path = "tests/vnc/keysym.rs"]
mod tests;
