    use super::*;

    fn stroke(key: &str, key_char: Option<&str>) -> Keystroke {
        Keystroke {
            modifiers: Modifiers::default(),
            key: key.into(),
            key_char: key_char.map(str::to_string),
        }
    }

    /// 按着修饰键的一下。`key_char` 按 macOS 的规矩给 `None` —— ctrl / cmd / fn
    /// 按着时那边就是不给。
    fn chord(modifiers: Modifiers, key: &str) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.into(),
            key_char: None,
        }
    }

    #[test]
    fn named_keys_beat_what_they_would_type() {
        assert_eq!(keysym(&stroke("return", None), true), Some(0xff0d));
        assert_eq!(keysym(&stroke("up", None), true), Some(0xff52));
        assert_eq!(keysym(&stroke("f1", None), true), Some(0xffbe));
        assert_eq!(keysym(&stroke("f12", None), true), Some(0xffc9));
        assert_eq!(keysym(&stroke("space", Some(" ")), true), Some(0x20));
        // 超出 f24 的 `f99` 不算具名键，键帽又不是一个字符：不发。
        assert_eq!(keysym(&stroke("f99", None), true), None);
    }

    #[test]
    fn typed_characters_follow_the_unicode_rule() {
        assert_eq!(keysym(&stroke("a", Some("a")), true), Some(0x61));
        assert_eq!(keysym(&stroke("1", Some("!")), true), Some(0x21));
        assert_eq!(keysym(&stroke("e", Some("é")), true), Some(0xe9));
        assert_eq!(keysym(&stroke("k", Some("中")), true), Some(0x0100_4e2d));
    }

    /// 组合键按键帽走。macOS 在 ctrl / cmd 按着时不给 `key_char`，只认它的话
    /// ⌃C、⌘S 一条 `KeyEvent` 都发不出去 —— 组合键"完全没法用"就是这么来的。
    #[test]
    fn chords_report_the_key_cap_even_without_a_typed_character() {
        assert_eq!(keysym(&chord(Modifiers::control(), "c"), false), Some(0x63));
        assert_eq!(keysym(&chord(Modifiers::command(), "s"), false), Some(0x73));
        // Ctrl+Alt+Del：具名键那一档照旧，修饰键由 `modifier_keysyms` 另发。
        let mut both = Modifiers::control();
        both.alt = true;
        assert_eq!(keysym(&chord(both, "delete"), false), Some(0xffff));
    }

    /// 组合键不看"打出来的字符"：⌥S 在美式键盘上打出来的是 `ß`，
    /// 人按的却是 Alt+S，对端要的也是 S。
    #[test]
    fn a_chord_ignores_what_the_local_layout_would_have_typed() {
        let mut keystroke = stroke("s", Some("ß"));
        keystroke.modifiers = Modifiers::alt();
        assert_eq!(keysym(&keystroke, false), Some(0x73));
        // 同一下若被点名成"在打字"（Windows 的 AltGr 那档），才按字符算。
        assert_eq!(keysym(&keystroke, true), Some(0xdf));
    }

    /// shift 按着的字母报大写，对端收到的是 Ctrl+Shift+A。
    #[test]
    fn shift_upgrades_the_key_cap_to_its_capital() {
        assert_eq!(
            keysym(&chord(Modifiers::control_shift(), "a"), false),
            Some(0x41)
        );
        assert_eq!(keysym(&chord(Modifiers::shift(), "a"), false), Some(0x41));
    }

    #[test]
    fn modifier_state_spreads_into_keysyms() {
        let modifiers = Modifiers {
            control: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(modifier_keysyms(&modifiers), vec![SHIFT, CONTROL]);
        assert!(is_modifier(SHIFT));
        assert!(!is_modifier(0xff0d));
    }
