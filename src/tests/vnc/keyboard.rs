    use super::*;

    /// 把一串 [`VncInput`] 摊成好断言的 `(keysym, down)`。
    fn pressed(inputs: Vec<VncInput>) -> Vec<(u32, bool)> {
        inputs
            .into_iter()
            .map(|input| match input {
                VncInput::Key { keysym, down } => (keysym, down),
                other => panic!("键盘只该发 KeyEvent，发出来的是 {other:?}"),
            })
            .collect()
    }

    fn stroke(modifiers: Modifiers, key: &str, key_char: Option<&str>) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.into(),
            key_char: key_char.map(str::to_string),
        }
    }

    /// 一次 Ctrl+C 的全程：Ctrl 按下、c 按下抬起、Ctrl 抬起，四条各一次。
    /// 组合键坏掉的那版这里只有前后两条修饰键 —— `c` 一条都没有。
    #[test]
    fn a_chord_sends_the_modifier_and_the_key_around_it() {
        let mut keyboard = VncKeyboard::new();
        assert_eq!(
            pressed(keyboard.modifiers(&Modifiers::control())),
            vec![(keysym::CONTROL, true)]
        );
        assert_eq!(
            pressed(keyboard.down(&stroke(Modifiers::control(), "c", None), false)),
            vec![(0x63, true)]
        );
        assert_eq!(pressed(keyboard.up("c")), vec![(0x63, false)]);
        assert_eq!(
            pressed(keyboard.modifiers(&Modifiers::none())),
            vec![(keysym::CONTROL, false)]
        );
    }

    /// 修饰键抬起只有 `ModifiersChangedEvent` 报得出来。没有这一条，
    /// Ctrl 会一直按在对端那边，之后每个字母都变成组合键 —— 用一次组合键
    /// 就毁掉整条会话。
    #[test]
    fn letting_go_of_a_modifier_reaches_the_far_side() {
        let mut keyboard = VncKeyboard::new();
        keyboard.modifiers(&Modifiers::control());
        keyboard.down(&stroke(Modifiers::control(), "c", None), false);
        keyboard.up("c");

        assert_eq!(
            pressed(keyboard.modifiers(&Modifiers::none())),
            vec![(keysym::CONTROL, false)]
        );
        // 已经归零了就不再重复发。
        assert!(keyboard.modifiers(&Modifiers::none()).is_empty());
    }

    /// 鼠标事件那份修饰键只补不松：补一颗对端还不知道按着的稳赚，
    /// 拿它松键则会误伤 —— macOS 把 Control+左键换成右键时，事件里那颗
    /// Control 不保证还在，照它松就把这一下正要用的 Control 松掉了
    /// （多选于是变成了右键菜单）。
    #[test]
    fn a_mouse_event_may_press_modifiers_but_never_lets_them_go() {
        let mut keyboard = VncKeyboard::new();
        assert_eq!(
            pressed(keyboard.hold(&Modifiers::control())),
            vec![(keysym::CONTROL, true)]
        );
        assert!(keyboard.holds_control());

        // 已经按着就不重复补。
        assert!(keyboard.hold(&Modifiers::control()).is_empty());
        // 这一下没报 Control，也不松 —— 松不松只听键盘那条。
        assert!(keyboard.hold(&Modifiers::none()).is_empty());
        assert!(keyboard.holds_control(), "鼠标事件松不掉修饰键");

        assert_eq!(
            pressed(keyboard.modifiers(&Modifiers::none())),
            vec![(keysym::CONTROL, false)]
        );
        assert!(!keyboard.holds_control());
    }

    /// 按住不放：每一下重复都照发，账上仍然只有一颗键，抬起也只有一条。
    #[test]
    fn auto_repeat_keeps_sending_but_only_owes_one_release() {
        let mut keyboard = VncKeyboard::new();
        let keystroke = stroke(Modifiers::none(), "a", Some("a"));
        for _ in 0..3 {
            assert_eq!(pressed(keyboard.down(&keystroke, true)), vec![(0x61, true)]);
        }
        assert_eq!(pressed(keyboard.up("a")), vec![(0x61, false)]);
        assert!(keyboard.keys.is_empty());
    }

    /// 抬起抬的是按下时发的那颗。按住 `1` 再按 shift，键帽从 `1` 变成 `!`：
    /// 账上换成新的那颗，旧的当场抬掉，两颗都不会留在对端。
    #[test]
    fn a_key_that_changes_its_keysym_mid_press_releases_the_old_one() {
        let mut keyboard = VncKeyboard::new();
        keyboard.down(&stroke(Modifiers::none(), "1", Some("1")), true);
        assert_eq!(
            pressed(keyboard.down(&stroke(Modifiers::shift(), "1", Some("!")), true)),
            vec![(keysym::SHIFT, true), (0x31, false), (0x21, true)]
        );
        assert_eq!(pressed(keyboard.up("1")), vec![(0x21, false)]);
    }

    /// 按下与抬起之间松了 shift，键帽的名字跟着变（mac 上 `!` → `1`），
    /// 认不出来的抬起就把账上按着的全松掉 —— 宁可多松一次，也不留一颗
    /// 永远按着的键。
    #[test]
    fn an_unclaimed_release_lets_go_of_everything_still_held() {
        let mut keyboard = VncKeyboard::new();
        keyboard.down(&stroke(Modifiers::shift(), "!", Some("!")), true);
        assert_eq!(pressed(keyboard.up("1")), vec![(0x21, false)]);
        assert!(keyboard.keys.is_empty());
    }

    /// 失焦、切窗：按着的普通键与修饰键一起还回去。
    #[test]
    fn losing_focus_gives_every_held_key_back() {
        let mut keyboard = VncKeyboard::new();
        keyboard.modifiers(&Modifiers::control());
        keyboard.down(&stroke(Modifiers::control(), "a", None), false);

        assert_eq!(
            pressed(keyboard.release()),
            vec![(0x61, false), (keysym::CONTROL, false)]
        );
        assert!(keyboard.release().is_empty(), "松过一次就不欠了");
    }

    /// 换一条会话只清账：那些键按在一台我们再也说不上话的对端上，
    /// 往新的那条发抬起等于凭空按了一下又松开。
    #[test]
    fn a_new_session_starts_from_an_empty_slate() {
        let mut keyboard = VncKeyboard::new();
        keyboard.modifiers(&Modifiers::control());
        keyboard.down(&stroke(Modifiers::control(), "a", None), false);

        keyboard.forget();
        assert!(keyboard.release().is_empty());
    }

    /// macOS 上 ⌘ 按着时系统不派发 keyUp，按下的同一拍就得把抬起补上。
    #[cfg(target_os = "macos")]
    #[test]
    fn a_command_chord_releases_itself_because_no_key_up_is_coming() {
        let mut keyboard = VncKeyboard::new();
        keyboard.modifiers(&Modifiers::command());
        assert_eq!(
            pressed(keyboard.down(&stroke(Modifiers::command(), "s", None), false)),
            vec![(0x73, true), (0x73, false)]
        );
        assert!(keyboard.keys.is_empty(), "补过抬起就不该再欠着");
    }

    /// 其余平台的 Super 组合键成对派发，按下就只发按下。
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_super_chord_waits_for_its_key_up() {
        let mut keyboard = VncKeyboard::new();
        keyboard.modifiers(&Modifiers::command());
        assert_eq!(
            pressed(keyboard.down(&stroke(Modifiers::command(), "s", None), false)),
            vec![(0x73, true)]
        );
        assert_eq!(pressed(keyboard.up("s")), vec![(0x73, false)]);
    }
