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
        assert!(keyboard.holds_control());
        assert!(keyboard.hold(&Modifiers::control()).is_empty());
        assert!(keyboard.hold(&Modifiers::none()).is_empty());
        assert!(keyboard.holds_control());

        assert_eq!(
            keys(keyboard.modifiers(&Modifiers::none())),
            vec![("ctrl", false)]
        );
        assert!(!keyboard.holds_control());
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
        assert_eq!(
            keys(keyboard.modifiers(&Modifiers::command())),
            vec![("super", true)]
        );
        assert_eq!(
            keys(keyboard.down(&stroke(Modifiers::command(), "s", None), false)),
            vec![("0x1F", true), ("0x1F", false)]
        );
        assert!(keyboard.keys.is_empty(), "补过抬起就不该再欠着");
    }