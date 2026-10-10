    use super::{describe, parse_octal};

    #[test]
    fn octal_accepts_the_ways_people_write_it() {
        assert_eq!(parse_octal("644"), Some(0o644));
        assert_eq!(parse_octal(" 755 "), Some(0o755));
        assert_eq!(parse_octal("0644"), Some(0o644));
        assert_eq!(parse_octal("4755"), Some(0o4755), "setuid 那一档允许写");
        assert_eq!(parse_octal("0"), None, "一位数说不清是哪种档位");
        assert_eq!(parse_octal("77777"), None, "越过 07777");
        assert_eq!(parse_octal("999"), None, "八进制里没有 9");
        assert_eq!(parse_octal("75a"), None);
        assert_eq!(parse_octal(""), None);
    }

    /// 常点的那几档各有各的说法，冷门的也有一句兜底。
    ///
    /// 不比具体那句话：文案在 `locales/app.yml` 里，跟着语言变。这里看的是
    /// "每一档都说了话，而且说的不是同一句" —— 漏一档 `match` 分支就会让两个
    /// 模式撞到同一句兜底上，那正是这个断言拦得住的。
    #[test]
    fn the_common_modes_explain_themselves() {
        let common = [0o444, 0o600, 0o644, 0o666, 0o700, 0o755, 0o777];
        let fallback = describe(0o123);
        assert!(!fallback.is_empty(), "冷门的也要有一句话");

        let mut said = std::collections::BTreeSet::new();
        for mode in common {
            let text = describe(mode);
            assert_ne!(text, fallback, "{mode:o} 落到了兜底那一句");
            assert!(said.insert(text), "{mode:o} 和别的档位说了同一句话");
        }
    }
