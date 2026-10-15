//! 键盘此刻在对端那边按成什么样。
//!
//! RFB 的键盘是一台**状态机**：协议里只有"这颗 keysym 按下了 / 抬起了"，
//! 没有"按着 shift 的 a"这种说法，也没有人替你兜底 —— 一条发漏的抬起会让
//! 那颗键在对端一直按着，从此每个字母都是组合键。而 GPUI 给的是另一套东西：
//! 修饰键只报状态不报过程，抬起事件有时候压根不来。中间这本账就记在这里，
//! 由 [`crate::ui::panels::VncPanel`] 把算出来的每一条 [`VncInput`] 发走。
//!
//! 记两样：
//!
//! - **修饰键**（`Shift_L`、`Control_L`……）：GPUI 的 `Modifiers` 是一张此刻
//!   的状态表，对着上一张发差值就是按下与抬起。
//! - **普通键**：键帽名字 → 按下时发出去的那颗 keysym。抬起时抬的是账上
//!   那颗，不是抬起那一下现算的 —— 两者会不一样，见 [`VncKeyboard::up`]。
//!
//! 翻译本身（键 → keysym）归 [`super::keysym`]，这里只管"发几条、发哪些"。

use gpui_kit::{Keystroke, Modifiers};

use super::VncInput;
use super::keysym;

/// ⌘（platform 修饰键）按着的时候，系统还派不派 keyUp。
///
/// macOS 不派：⌘ 组合键走的是 `performKeyEquivalent:`，AppKit 一直没有配对的
/// 抬起那一半。等不来的抬起只能在按下的同一拍自己补上，否则 ⌘C 一下就能把
/// `c` 永远按在对端。其余平台正常成对。
const PLATFORM_SWALLOWS_KEY_UP: bool = cfg!(target_os = "macos");

#[derive(Default)]
pub struct VncKeyboard {
    /// 此刻在对端那边按着的修饰键 keysym。
    modifiers: Vec<u32>,
    /// 此刻在对端那边按着的普通键：键帽名字 → 按下时发出去的那颗 keysym。
    keys: Vec<(String, u32)>,
}

impl VncKeyboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 把对端的修饰键对齐到 `modifiers`：松开的先松、新按的再按，对端看到的
    /// 状态于是恰好等于此刻本地按着的那组。
    ///
    /// 这件事要挂在 `ModifiersChangedEvent` 上而不是只在击键时做：按下还能从
    /// 击键里看出来，**抬起**只有它报 —— 光对击键的差值，松开 Ctrl 这件事
    /// 永远等不到下一次击键。
    pub fn modifiers(&mut self, modifiers: &Modifiers) -> Vec<VncInput> {
        let now = keysym::modifier_keysyms(modifiers);
        if now == self.modifiers {
            return Vec::new();
        }
        let mut inputs = Vec::new();
        for left in self.modifiers.iter().filter(|held| !now.contains(held)) {
            inputs.push(key(*left, false));
        }
        for entered in now.iter().filter(|held| !self.modifiers.contains(held)) {
            inputs.push(key(*entered, true));
        }
        self.modifiers = now;
        inputs
    }

    /// 只补按下、不补抬起的一次对齐。鼠标事件用。
    ///
    /// 鼠标事件也带着一份修饰键状态，但它不如键盘那条可靠：macOS 把"按着
    /// Control 的左键"换成右键的时候，事件里那颗 Control 不保证还在。拿这份
    /// 状态去**松**键会误伤 —— 松掉的恰恰是这一下正要用的那颗（Ctrl+左键
    /// 多选就是这么没的）；拿它**补**一颗"对端还不知道按着的"修饰键则稳赚：
    /// 真松开了，键盘那条 `ModifiersChangedEvent` 会来收尾。
    pub fn hold(&mut self, modifiers: &Modifiers) -> Vec<VncInput> {
        let mut inputs = Vec::new();
        for entered in keysym::modifier_keysyms(modifiers) {
            if !self.modifiers.contains(&entered) {
                self.modifiers.push(entered);
                inputs.push(key(entered, true));
            }
        }
        inputs
    }

    /// 对端那边此刻按着 Control 吗。
    ///
    /// 鼠标那一路拿它当"人是不是正按着 Control"的第二个说法：键盘事件报的
    /// 状态和鼠标事件报的是两个来源，后者在 macOS 的 Control+左键上会丢，
    /// 前者不会。
    pub fn holds_control(&self) -> bool {
        self.modifiers.contains(&keysym::CONTROL)
    }

    /// 一次击键。`typing` 是"这一下按它打出来的字符算"，见
    /// [`keysym::keysym`]。
    ///
    /// 击键自己带的修饰键状态也顺手对一遍：焦点是在修饰键已经按着的时候进来的
    /// （按着 Ctrl 点一下画布）时，那次按下的事件这块面板没赶上。
    ///
    /// 同一颗键帽再按一次是按住不放的自动重复，照发不误 —— 对端自己按它的
    /// 节奏处理重复，本地不该替它省。
    pub fn down(&mut self, keystroke: &Keystroke, typing: bool) -> Vec<VncInput> {
        let mut inputs = self.modifiers(&keystroke.modifiers);

        let Some(symbol) = keysym::keysym(keystroke, typing) else {
            return inputs;
        };
        // 键帽自己到场的修饰键（单独按下 shift）已经由上面的差值报过，
        // 不再重复一条 —— 重复的按下没事，配对的抬起丢了才要命。
        if keysym::is_modifier(symbol) {
            return inputs;
        }

        let cap = &keystroke.key;
        match self.keys.iter().position(|(held, _)| held == cap) {
            // 按着不放的中途改了修饰键（按住 `1` 再按 shift）：这颗键帽换了
            // keysym，账上那颗旧的先抬掉，不然它留在对端那边。
            Some(at) if self.keys[at].1 != symbol => {
                inputs.push(key(self.keys[at].1, false));
                self.keys[at].1 = symbol;
            }
            Some(_) => {}
            None => self.keys.push((keystroke.key.clone(), symbol)),
        }
        inputs.push(key(symbol, true));

        if PLATFORM_SWALLOWS_KEY_UP && keystroke.modifiers.platform {
            inputs.extend(self.up(&keystroke.key));
        }
        inputs
    }

    /// 一颗键帽抬起来：抬的是它按下时发出去的那颗 keysym。
    ///
    /// 账上没有这颗键帽，说明这一对按下抬起对不上了 —— 按下与抬起之间修饰键
    /// 变了，键帽的名字跟着变（mac 上 `shift-1` 按下时键帽叫 `!`，先松 shift
    /// 再松键，抬起那下就叫 `1`）。这时候账上按着的都已经没人认领了，一起松掉：
    /// 宁可多松一次（对端收到一条没配对的抬起，无事发生），也不留一颗永远按着
    /// 的键。
    pub fn up(&mut self, cap: &str) -> Vec<VncInput> {
        match self.keys.iter().position(|(held, _)| held == cap) {
            Some(at) => vec![key(self.keys.remove(at).1, false)],
            None => self.release_keys(),
        }
    }

    /// 键盘整个松手：普通键和修饰键都还给对端。失焦、切窗时用 —— 手是在我们
    /// 看不见的地方松的（⌘Tab 切走时那颗 ⌘ 的抬起进的是别人的窗），对端只能
    /// 靠这一下把键盘状态归零。
    pub fn release(&mut self) -> Vec<VncInput> {
        let mut inputs = self.release_keys();
        for symbol in std::mem::take(&mut self.modifiers) {
            inputs.push(key(symbol, false));
        }
        inputs
    }

    /// 换一条会话：只把账清掉，一条抬起都不发 —— 那些键按在一台我们再也说不上
    /// 话的对端上。
    pub fn forget(&mut self) {
        self.modifiers.clear();
        self.keys.clear();
    }

    fn release_keys(&mut self) -> Vec<VncInput> {
        std::mem::take(&mut self.keys)
            .into_iter()
            .map(|(_, symbol)| key(symbol, false))
            .collect()
    }
}

fn key(keysym: u32, down: bool) -> VncInput {
    VncInput::Key { keysym, down }
}

#[cfg(test)]
#[path = "tests/vnc/keyboard.rs"]
mod tests;
