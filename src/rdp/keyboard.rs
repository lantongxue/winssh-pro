//! GPUI 按键 → RDP 输入事件。
//!
//! RDP 的键盘走的是 **PC/AT 扫描码**（set 1）：协议里报的是"哪一颗物理键
//! 按下了 / 抬起了"，布局与大小写在服务器那边按它自己的键表翻。<-[`Scancode`]
//! 是 `ironrdp-input` 的类型，`extended` 那面旗就是 E0 前缀（方向键、右 Ctrl、
//! 小键盘回车这些）。
//!
//! 翻译按这个顺序，和 [`crate::vnc::keysym`] 三步是同一个道理：
//!
//! 1. **具名键**查表（方向键、回车、功能键、编辑键）—— 这些没有"打出来的
//!    字符"，只有扫描码；
//! 2. **在打字**（没按住 ctrl / alt / cmd / fn）就发送 Unicode 事件
//!    （`FASTPATH_UNICODE_KEYBOARD_EVENT`）：把 `key_char` 里的字符直接交给
//!    对端。本地布局与远端布局不一致时，扫描码会把字母送到**物理位置**上
//!    （AZERTY 的 Q 位在美式表里是 `q`），Unicode 事件送的才是人打出来的那个字；
//!    中文输入法落下来的字也从这条路过得去；
//! 3. **在按组合键**就按键帽 `key` 走美式扫描码：Ctrl+C 这类组合要的是键帽，
//!    不是本地布局在那一格上放的是什么字。
//!
//! [`Scancode`]: ironrdp::input::Scancode
//!
//! 修饰键（Shift/Ctrl/Alt/Win）单独说：GPUI 只在 `Keystroke.modifiers` 里
//! 给状态不给过程，所以由 [`RdpKeyboard::modifiers`] 对着上一次的集合发差值，
//! 抬起这件事只有 `ModifiersChangedEvent` 报得出来。
//!
//! 发出去的是 [`Operation`]（`ironrdp-input` 的高层动作），状态机在
//! [`crate::rdp::RdpSession`] 那头的 `Database` 里 —— 按键的去重、自动重复的
//! 补释放、`release_all` 都由它兜底，这里只负责"人做了什么"。

use gpui_kit::{Keystroke, Modifiers};
use ironrdp::input::{Operation, Scancode};

/// macOS 上 ⌘ 按着时系统不派 keyUp（与 VNC 那条同一个坑，见
/// [`crate::vnc::keyboard`]）。按下的同一拍自己补抬起。
const PLATFORM_SWALLOWS_KEY_UP: bool = cfg!(target_os = "macos");

/// 此刻在对端那边按着的修饰键扫描码。
const SHIFT: Scancode = Scancode::from_u8(false, 0x2A);
const CONTROL: Scancode = Scancode::from_u8(false, 0x1D);
const ALT: Scancode = Scancode::from_u8(false, 0x38);
/// 左 Win / Super。RDP 那边真能打开开始菜单，这是远程桌面的正常行为。
const SUPER: Scancode = Scancode::from_u8(true, 0x5B);

/// 这颗键帽按下时发出去的是什么。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sent {
    Scancode(Scancode),
    Unicode(char),
}

#[derive(Default)]
pub struct RdpKeyboard {
    /// 此刻在对端那边按着的修饰键。
    modifiers: Vec<Scancode>,
    /// 此刻在对端那边按着的普通键：键帽名字 → 按下时发出去的东西。
    keys: Vec<(String, Sent)>,
}

impl RdpKeyboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 把对端的修饰键对齐到 `modifiers`：松开的先松、新按的再按。
    pub fn modifiers(&mut self, modifiers: &Modifiers) -> Vec<Operation> {
        let now = modifier_scancodes(modifiers);
        if now == self.modifiers {
            return Vec::new();
        }
        let mut operations = Vec::new();
        for left in self.modifiers.iter().filter(|held| !now.contains(held)) {
            operations.push(key(*left, false));
        }
        for entered in now.iter().filter(|held| !self.modifiers.contains(held)) {
            operations.push(key(*entered, true));
        }
        self.modifiers = now;
        operations
    }

    /// 只补按下、不补抬起的一次对齐。鼠标事件用 —— 理由与
    /// [`crate::vnc::VncKeyboard::hold`] 相同：鼠标事件里的修饰键状态
    /// 不可靠，拿它**松**键会误伤，拿它**补**键稳赚。
    pub fn hold(&mut self, modifiers: &Modifiers) -> Vec<Operation> {
        let mut operations = Vec::new();
        for entered in modifier_scancodes(modifiers) {
            if !self.modifiers.contains(&entered) {
                self.modifiers.push(entered);
                operations.push(key(entered, true));
            }
        }
        operations
    }

    /// 一次击键。`typing` 是"这一下按它打出来的字符算"（见模块文档第 2 步）。
    pub fn down(&mut self, keystroke: &Keystroke, typing: bool) -> Vec<Operation> {
        let mut operations = self.modifiers(&keystroke.modifiers);

        let sent = if let Some(scancode) = named_scancode(&keystroke.key) {
            if is_modifier(scancode) {
                // 键帽自己到场的修饰键已经由上面的差值报过，不重复。
                return operations;
            }
            Sent::Scancode(scancode)
        } else if typing
            && let Some(character) = keystroke
                .key_char
                .as_deref()
                .and_then(|text| text.chars().next())
        {
            Sent::Unicode(character)
        } else if let Some(scancode) = cap_scancode(&keystroke.key) {
            Sent::Scancode(scancode)
        } else {
            return operations;
        };

        let cap = &keystroke.key;
        match self.keys.iter().position(|(held, _)| held == cap) {
            // 按着不放的中途换了修饰键（按住 `1` 再按 shift）：键帽没变，
            // 发出去的东西可能变了，旧的先抬掉。
            Some(at) if self.keys[at].1 != sent => {
                operations.push(release(self.keys[at].1));
                self.keys[at].1 = sent;
            }
            Some(_) => {}
            None => self.keys.push((keystroke.key.clone(), sent)),
        }
        operations.push(press(sent));

        if PLATFORM_SWALLOWS_KEY_UP && keystroke.modifiers.platform {
            operations.extend(self.up(&keystroke.key));
        }
        operations
    }

    /// 一颗键帽抬起来：抬的是它按下时发出去的那一样。
    ///
    /// 账上没有这颗键帽说明这一对按下抬起对不上了（按下与抬起之间修饰键变了，
    /// 键帽名字跟着变）。这时候账上按着的都已经没人认领，一起松掉 ——
    /// 宁可多松一次，也不留一颗永远按着的键。
    pub fn up(&mut self, cap: &str) -> Vec<Operation> {
        match self.keys.iter().position(|(held, _)| held == cap) {
            Some(at) => vec![release(self.keys.remove(at).1)],
            None => self.release_keys(),
        }
    }

    /// 换一条会话：只清账，不发抬起 —— 那些键按在一台我们再也说不上话的
    /// 对端上。失焦不在这儿：那是 [`super::RdpInput::ReleaseAll`] 的活，
    /// 会话那头的 `Database` 把按着的键鼠与 Unicode 一次结清。
    pub fn forget(&mut self) {
        self.modifiers.clear();
        self.keys.clear();
    }

    fn release_keys(&mut self) -> Vec<Operation> {
        std::mem::take(&mut self.keys)
            .into_iter()
            .map(|(_, sent)| release(sent))
            .collect()
    }
}

fn press(sent: Sent) -> Operation {
    match sent {
        Sent::Scancode(scancode) => key(scancode, true),
        Sent::Unicode(character) => Operation::UnicodeKeyPressed(character),
    }
}

fn release(sent: Sent) -> Operation {
    match sent {
        Sent::Scancode(scancode) => key(scancode, false),
        Sent::Unicode(character) => Operation::UnicodeKeyReleased(character),
    }
}

fn key(scancode: Scancode, down: bool) -> Operation {
    match down {
        true => Operation::KeyPressed(scancode),
        false => Operation::KeyReleased(scancode),
    }
}

/// 此刻按着的修饰键（GPUI 状态 → 扫描码集合）。左右手一律报左手那颗。
fn modifier_scancodes(modifiers: &Modifiers) -> Vec<Scancode> {
    let mut scancodes = Vec::with_capacity(4);
    if modifiers.shift {
        scancodes.push(SHIFT);
    }
    if modifiers.control {
        scancodes.push(CONTROL);
    }
    if modifiers.alt {
        scancodes.push(ALT);
    }
    if modifiers.platform {
        scancodes.push(SUPER);
    }
    scancodes
}

/// 这个扫描码是不是修饰键。
fn is_modifier(scancode: Scancode) -> bool {
    scancode == SHIFT || scancode == CONTROL || scancode == ALT || scancode == SUPER
}

/// 具名键表。键是 `Keystroke.key`（GPUI 统一转小写）。
fn named_scancode(key: &str) -> Option<Scancode> {
    let scancode = match key {
        "escape" | "esc" => Scancode::from_u8(false, 0x01),
        "tab" => Scancode::from_u8(false, 0x0F),
        "return" | "enter" => Scancode::from_u8(false, 0x1C),
        "backspace" => Scancode::from_u8(false, 0x0E),
        "capslock" => Scancode::from_u8(false, 0x3A),
        "numlock" => Scancode::from_u8(false, 0x45),
        "scrolllock" => Scancode::from_u8(false, 0x46),
        "space" => Scancode::from_u8(false, 0x39),
        "printscreen" => Scancode::from_u8(true, 0x37),
        "insert" => Scancode::from_u8(true, 0x52),
        "delete" => Scancode::from_u8(true, 0x53),
        "home" => Scancode::from_u8(true, 0x47),
        "end" => Scancode::from_u8(true, 0x4F),
        "pageup" | "prior" => Scancode::from_u8(true, 0x49),
        "pagedown" | "next" => Scancode::from_u8(true, 0x51),
        "left" => Scancode::from_u8(true, 0x4B),
        "up" => Scancode::from_u8(true, 0x48),
        "right" => Scancode::from_u8(true, 0x4D),
        "down" => Scancode::from_u8(true, 0x50),
        "menu" | "contextmenu" => Scancode::from_u8(true, 0x5D),
        "shift" => SHIFT,
        "control" | "ctrl" => CONTROL,
        "alt" | "option" => ALT,
        "cmd" | "command" | "platform" | "super" | "win" => SUPER,
        _ => return function_scancode(key),
    };
    Some(scancode)
}

/// `f1`–`f12`：set 1 的扫描码是连号的（f1 = 0x3B），f11/f12 另在两格。
fn function_scancode(key: &str) -> Option<Scancode> {
    let number = key.strip_prefix('f')?.parse::<u32>().ok()?;
    match number {
        1..=10 => Some(Scancode::from_u8(false, 0x3B + (number - 1) as u8)),
        11 => Some(Scancode::from_u8(false, 0x57)),
        12 => Some(Scancode::from_u8(false, 0x58)),
        _ => None,
    }
}

/// 键帽字符 → 美式布局的扫描码。组合键走的就是这条路（见模块文档第 3 步），
/// 大小写在这里不参与 —— shift 的状态是修饰键那条账在管。
fn cap_scancode(key: &str) -> Option<Scancode> {
    let mut characters = key.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    let character = character.to_ascii_lowercase();
    let code = match character {
        'a'..='z' => match character {
            'a' => 0x1E,
            'b' => 0x30,
            'c' => 0x2E,
            'd' => 0x20,
            'e' => 0x12,
            'f' => 0x21,
            'g' => 0x22,
            'h' => 0x23,
            'i' => 0x17,
            'j' => 0x24,
            'k' => 0x25,
            'l' => 0x26,
            'm' => 0x32,
            'n' => 0x31,
            'o' => 0x18,
            'p' => 0x19,
            'q' => 0x10,
            'r' => 0x13,
            's' => 0x1F,
            't' => 0x14,
            'u' => 0x16,
            'v' => 0x2F,
            'w' => 0x11,
            'x' => 0x2D,
            'y' => 0x15,
            'z' => 0x2C,
            _ => return None,
        },
        '1'..='9' => 0x02 + (character as u8 - b'1'),
        '0' => 0x0B,
        '-' => 0x0C,
        '=' => 0x0D,
        '[' => 0x1A,
        ']' => 0x1B,
        ';' => 0x27,
        '\'' => 0x28,
        '`' => 0x29,
        '\\' => 0x2B,
        ',' => 0x33,
        '.' => 0x34,
        '/' => 0x35,
        _ => return None,
    };
    Some(Scancode::from_u8(false, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把一串 [`Operation`] 摊成好断言的 `(键帽, 按下)`。
    fn keys(operations: Vec<Operation>) -> Vec<(&'static str, bool)> {
        operations
            .into_iter()
            .map(|operation| match operation {
                Operation::KeyPressed(scancode) => (name(scancode), true),
                Operation::KeyReleased(scancode) => (name(scancode), false),
                Operation::UnicodeKeyPressed(character) => (leak(character), true),
                Operation::UnicodeKeyReleased(character) => (leak(character), false),
                other => panic!("键盘只该发按键，发出来的是 {other:?}"),
            })
            .collect()
    }

    fn leak(character: char) -> &'static str {
        Box::leak(character.to_string().into_boxed_str())
    }

    fn name(scancode: Scancode) -> &'static str {
        match scancode {
            SHIFT => "shift",
            CONTROL => "ctrl",
            ALT => "alt",
            SUPER => "super",
            other => match other.as_u8() {
                (false, 0x1E) => "a",
                (false, 0x1C) => "enter",
                (true, 0x53) => "delete",
                (true, code) => panic!("没命名过的扩展键 0xE0{code:02X}"),
                (false, code) => Box::leak(format!("0x{code:02X}").into_boxed_str()),
            },
        }
    }

    fn stroke(modifiers: Modifiers, key: &str, key_char: Option<&str>) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.into(),
            key_char: key_char.map(str::to_string),
        }
    }

    /// 一次 Ctrl+C：Ctrl 按下、`c` 一条**扫描码**（组合键按键帽走），
    /// Ctrl 抬起。c 不能走 Unicode，否则对端收到的是"打出来的 c"而不是
    /// "Ctrl 按着的 C"，复制就成了往终端里敲字。
    #[test]
    fn a_chord_sends_the_modifier_and_the_key_cap() {
        let mut keyboard = RdpKeyboard::new();
        assert_eq!(
            keys(keyboard.modifiers(&Modifiers::control())),
            vec![("ctrl", true)]
        );
        assert_eq!(
            keys(keyboard.down(&stroke(Modifiers::control(), "c", None), false)),
            vec![("0x2E", true)]
        );
        assert_eq!(keys(keyboard.up("c")), vec![("0x2E", false)]);
        assert_eq!(
            keys(keyboard.modifiers(&Modifiers::none())),
            vec![("ctrl", false)]
        );
    }

    /// 打字走 Unicode 事件：local 布局（AZERTY 的 Q 位）与远端（美式）不一致
    /// 时，扫描码会把 `q` 送到 `a` 的位置，Unicode 送的是人打出来的那个字。
    #[test]
    fn plain_typing_goes_through_unicode_events() {
        let mut keyboard = RdpKeyboard::new();
        assert_eq!(
            keys(keyboard.down(&stroke(Modifiers::none(), "a", Some("a")), true)),
            vec![("a", true)]
        );
        assert_eq!(keys(keyboard.up("a")), vec![("a", false)]);
    }

    /// 具名键没有"打出来的字符"，永远走扫描码。
    #[test]
    fn named_keys_always_use_scancodes() {
        let mut keyboard = RdpKeyboard::new();
        assert_eq!(
            keys(keyboard.down(&stroke(Modifiers::none(), "delete", None), true)),
            vec![("delete", true)]
        );
        assert_eq!(keys(keyboard.up("delete")), vec![("delete", false)]);

        let mut keyboard = RdpKeyboard::new();
        assert_eq!(
            keys(keyboard.down(&stroke(Modifiers::none(), "enter", None), true)),
            vec![("enter", true)]
        );
    }

    /// shift 按着打字：修饰键先报，字符本身仍然是打出来的那个大写。
    #[test]
    fn shift_upgrades_the_typed_character() {
        let mut keyboard = RdpKeyboard::new();
        assert_eq!(
            keys(keyboard.down(&stroke(Modifiers::shift(), "a", Some("A")), true)),
            vec![("shift", true), ("A", true)]
        );
    }

    /// 抬起抬的是按下时发的那个字符。按下与抬起之间松了 shift，
    /// 键帽名字从 `A` 变回 `a`，认不出来的抬起把账上按着的全松掉。
    #[test]
    fn an_unclaimed_release_lets_go_of_everything_still_held() {
        let mut keyboard = RdpKeyboard::new();
        keyboard.down(&stroke(Modifiers::shift(), "a", Some("A")), true);
        assert_eq!(keys(keyboard.up("a")), vec![("A", false)]);
        assert!(keyboard.keys.is_empty());
    }

    /// 组合键不看"本地布局会打出什么"：⌥S 在美式键盘上打出来是 `ß`，
    /// 人按的却是 Alt+S，对端要的也是 S。
    #[test]
    fn a_chord_ignores_what_the_local_layout_would_have_typed() {
        let mut keyboard = RdpKeyboard::new();
        let operations = keyboard.down(&stroke(Modifiers::alt(), "s", Some("ß")), false);
        assert_eq!(keys(operations), vec![("alt", true), ("0x1F", true)]);
    }

    /// 修饰键抬起只有 `ModifiersChangedEvent` 报得出来。
    #[test]
    fn letting_go_of_a_modifier_reaches_the_far_side() {
        let mut keyboard = RdpKeyboard::new();
        keyboard.modifiers(&Modifiers::control());
        keyboard.down(&stroke(Modifiers::control(), "c", None), false);
        keyboard.up("c");

        assert_eq!(
            keys(keyboard.modifiers(&Modifiers::none())),
            vec![("ctrl", false)]
        );
        assert!(keyboard.modifiers(&Modifiers::none()).is_empty());
    }

    /// 鼠标事件那份修饰键只补不松。
    #[test]
    fn a_mouse_event_may_press_modifiers_but_never_lets_them_go() {
        let mut keyboard = RdpKeyboard::new();
        assert_eq!(
            keys(keyboard.hold(&Modifiers::control())),
            vec![("ctrl", true)]
        );
        assert!(keyboard.hold(&Modifiers::control()).is_empty());
        assert!(keyboard.hold(&Modifiers::none()).is_empty());

        assert_eq!(
            keys(keyboard.modifiers(&Modifiers::none())),
            vec![("ctrl", false)]
        );
    }

    /// 失焦 / 换会话都只是清账：真正的松手由会话那头的 `Database`（`RdpInput::ReleaseAll`）
    /// 兜底，面板这本账不能留着一颗按着的键，否则回头会补发没人认领的抬起。
    #[test]
    fn clearing_the_ledger_remembers_nothing_held() {
        let mut keyboard = RdpKeyboard::new();
        keyboard.modifiers(&Modifiers::control());
        keyboard.down(&stroke(Modifiers::control(), "a", None), false);

        keyboard.forget();
        assert!(keyboard.keys.is_empty());
        assert!(keyboard.modifiers.is_empty());
    }

    /// 换一条会话只清账，不往新对端发抬起。
    #[test]
    fn a_new_session_starts_from_an_empty_slate() {
        let mut keyboard = RdpKeyboard::new();
        keyboard.modifiers(&Modifiers::control());
        keyboard.down(&stroke(Modifiers::control(), "a", None), false);

        keyboard.forget();
        // 账空了：对端已经换人，之前那颗 `a` 不再由我们负责。
        assert!(keyboard.up("a").is_empty(), "清过账就不欠对端一次抬起");
    }

    /// 扫描码表本身：字母按 QWERTY 排、数字 1–0 连号、方向键是扩展键。
    #[test]
    fn the_scancode_table_matches_the_us_layout() {
        assert_eq!(cap_scancode("q"), Some(Scancode::from_u8(false, 0x10)));
        assert_eq!(cap_scancode("Q"), Some(Scancode::from_u8(false, 0x10)));
        assert_eq!(cap_scancode("1"), Some(Scancode::from_u8(false, 0x02)));
        assert_eq!(cap_scancode("0"), Some(Scancode::from_u8(false, 0x0B)));
        assert_eq!(cap_scancode("/"), Some(Scancode::from_u8(false, 0x35)));
        assert_eq!(named_scancode("left"), Some(Scancode::from_u8(true, 0x4B)));
        assert_eq!(named_scancode("f12"), Some(Scancode::from_u8(false, 0x58)));
        assert_eq!(named_scancode("f99"), None);
        assert_eq!(cap_scancode("中"), None);
    }

    /// macOS 上 ⌘ 按着时系统不派 keyUp，按下的同一拍就得把抬起补上。
    #[cfg(target_os = "macos")]
    #[test]
    fn a_command_chord_releases_itself_because_no_key_up_is_coming() {
        let mut keyboard = RdpKeyboard::new();
        keyboard.modifiers(&Modifiers::command());
        assert_eq!(
            keys(keyboard.down(&stroke(Modifiers::command(), "s", None), false)),
            vec![("super", true), ("0x1F", true), ("0x1F", false)]
        );
        assert!(keyboard.keys.is_empty(), "补过抬起就不该再欠着");
    }
}