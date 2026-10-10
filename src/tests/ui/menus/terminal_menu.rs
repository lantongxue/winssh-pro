    use super::*;

    #[test]
    fn the_menu_never_fires_for_the_same_click_as_a_mouse_report() {
        // 一下右键要么归菜单、要么归对端，四种组合里每一种都必须正好落进一边。
        for mouse_mode in [false, true] {
            for shift in [false, true] {
                let menu = allowed(mouse_mode, shift);
                // 这是 `TerminalPanel::reports_mouse` 的判据，抄在这儿是为了
                // 两边一起改：其中一个变了而另一个没变，这个断言就会红。
                let reported = mouse_mode && !shift;
                assert_ne!(
                    menu, reported,
                    "mouse_mode={mouse_mode} shift={shift} 必须正好落进一边"
                );
            }
        }
    }

    #[test]
    fn shift_is_the_escape_hatch_while_the_app_owns_the_mouse() {
        assert!(allowed(false, false), "没开鼠标模式时右键就是菜单");
        assert!(allowed(false, true));
        assert!(!allowed(true, false), "开了鼠标模式时右键归对端");
        assert!(allowed(true, true), "Shift 把菜单抢回来");
    }

    #[test]
    fn copy_is_the_only_item_that_needs_a_selection() {
        for action in TerminalAction::ALL {
            assert!(action.enabled(true), "{action:?} 在有选中时该是可点的");
            assert_eq!(
                action.enabled(false),
                action != TerminalAction::Copy,
                "{action:?} 对选中的依赖判断错了"
            );
        }
    }

    #[test]
    fn the_items_fall_into_three_groups() {
        // 三段：选区相关、整屏相关、关掉会话。第一项不该自己起一段
        // —— 那会在菜单顶上多画一条线。
        let firsts: Vec<TerminalAction> = TerminalAction::ALL
            .into_iter()
            .filter(|action| action.starts_group())
            .collect();
        assert_eq!(firsts, [TerminalAction::CopyAll, TerminalAction::Close]);
        assert!(!TerminalAction::ALL[0].starts_group());
    }
