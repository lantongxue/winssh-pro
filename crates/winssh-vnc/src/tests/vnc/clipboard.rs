    use super::*;
    use vnc::clipboard::FORMAT_HTML;

    fn caps(formats: u32, actions: u32) -> Caps {
        Caps {
            formats,
            actions,
            limits: vec![(FORMAT_TEXT, 1 << 20)],
        }
    }

    fn extended() -> ClipboardSync {
        let mut sync = ClipboardSync::default();
        let reaction = sync.on_extended(ExtendedClipboard::Caps(caps(
            FORMAT_TEXT,
            ACTION_CAPS | ACTION_REQUEST | ACTION_NOTIFY | ACTION_PROVIDE,
        )));
        assert!(
            matches!(
                reaction.reply,
                Some(X11Event::Clipboard(ExtendedClipboard::Caps(_)))
            ),
            "对端递 Caps 该回敬一份 Caps",
        );
        sync
    }

    /// 自动档的发侧。
    fn auto_bytes(text: &str) -> Vec<u8> {
        classic_bytes(text, VncClipboardCharset::Auto)
    }

    /// 自动档的收侧，只要文本。
    fn auto_text(bytes: &[u8]) -> String {
        classic_text(bytes, VncClipboardCharset::Auto).text
    }

    #[test]
    fn classic_sends_latin1_when_it_fits_and_utf8_when_it_does_not() {
        assert_eq!(auto_bytes("git status"), b"git status");
        // é 是单字节 Latin-1：按规范读的对端与按 UTF-8 读的对端都能认出来。
        assert_eq!(auto_bytes("café"), vec![0x63, 0x61, 0x66, 0xE9]);
        // 中文装不进 Latin-1，只能按 UTF-8 发。
        assert_eq!(auto_bytes("你好"), "你好".as_bytes());
    }

    #[test]
    fn classic_line_endings_become_lf() {
        assert_eq!(auto_bytes("a\r\nb\rc"), b"a\nb\nc");
        assert_eq!(auto_bytes("a\nb"), b"a\nb");
    }

    #[test]
    fn classic_receives_utf8_or_latin1() {
        assert_eq!(auto_text("你好".as_bytes()), "你好");
        assert_eq!(auto_text(&[0x63, 0x61, 0x66, 0xE9]), "café");
        // 末尾的 NUL 不算文本。
        assert_eq!(auto_text(b"hi\0"), "hi");
    }

    /// 点了名的字符集两个方向都按那张表走 —— 中文 Windows 上的
    /// TightVNC / UltraVNC / RealVNC 就是这一档（GBK），繁体是 Big5。
    #[test]
    fn a_named_charset_is_used_in_both_directions() {
        // 你好 的 GBK 是 C4 E3 BA C3，和 UTF-8 的六个字节完全不同。
        let gbk = vec![0xC4, 0xE3, 0xBA, 0xC3];
        assert_eq!(classic_bytes("你好", VncClipboardCharset::Gbk), gbk);
        assert_eq!(
            classic_text(&gbk, VncClipboardCharset::Gbk).text,
            "你好",
            "点了 GBK 就该按 GBK 解，不该再按 UTF-8 猜",
        );

        // 同一串字节按 Big5 解是另一堆字：点名就是点名，不再嗅。
        let big5 = classic_bytes("中文", VncClipboardCharset::Big5);
        assert_eq!(big5, vec![0xA4, 0xA4, 0xA4, 0xE5]);
        assert_eq!(classic_text(&big5, VncClipboardCharset::Big5).text, "中文");

        // Shift_JIS 与 EUC-KR 也各自认得自己那一档。
        assert_eq!(
            classic_text(
                &classic_bytes("日本語", VncClipboardCharset::ShiftJis),
                VncClipboardCharset::ShiftJis,
            )
            .text,
            "日本語",
        );
        assert_eq!(
            classic_text(
                &classic_bytes("한국어", VncClipboardCharset::EucKr),
                VncClipboardCharset::EucKr,
            )
            .text,
            "한국어",
        );
    }

    /// 点名的那张表里没有位置的字换成问号，而不是 `&#128512;` 那种数字引用
    /// ——一个问号至少一眼能看出"这个字过不去"。
    #[test]
    fn a_character_the_named_charset_cannot_hold_becomes_a_question_mark() {
        assert_eq!(classic_bytes("hi😀", VncClipboardCharset::Gbk), b"hi?");
        // Latin-1 那一档同理（规范里就是一个字节一个码点）。
        assert_eq!(
            classic_bytes("café 你", VncClipboardCharset::Latin1),
            vec![0x63, 0x61, 0x66, 0xE9, 0x20, b'?']
        );
    }

    /// 自动档的嗅探：一份 GBK 的中文 UTF-8 解不动，严格试 GBK 解得通，
    /// 于是按 GBK 收下并记一笔；而一份 Latin-1 的西文不能被它抢走。
    #[test]
    fn the_auto_path_sniffs_gbk_without_stealing_latin1_text() {
        let gbk = vec![0xC4, 0xE3, 0xBA, 0xC3];
        assert_eq!(
            classic_text(&gbk, VncClipboardCharset::Auto),
            Decoded {
                text: "你好".into(),
                sniffed: Some(VncClipboardCharset::Gbk),
            },
        );

        // 重音字母连着出现（德文 ß、法文 é）：这些字节对在 GBK 里落在
        // 用户自定义区，严格解解不出来，于是照规范当 Latin-1。
        for (bytes, expected) in [
            (vec![0x47, 0x72, 0xFC, 0xDF, 0x65], "Grüße"),
            (vec![0x63, 0x61, 0x66, 0xE9], "café"),
            (vec![0xC0, 0xC9, 0xD6], "ÀÉÖ"),
            // 高字节夹在 ASCII 中间：GBK 表会把 EF 76 当成一个生僻字吃掉，
            // "高字节要成对"那一关正是为它设的。
            (vec![0x6E, 0x61, 0xEF, 0x76, 0x65], "naïve"),
        ] {
            assert_eq!(
                classic_text(&bytes, VncClipboardCharset::Auto),
                Decoded {
                    text: expected.into(),
                    sniffed: None,
                },
                "{expected:?} 是 Latin-1，不该被 GBK 抢走",
            );
        }

        // UTF-8 解得动就不再往下猜，也不记任何编码。
        assert_eq!(
            classic_text("你好".as_bytes(), VncClipboardCharset::Auto),
            Decoded {
                text: "你好".into(),
                sniffed: None,
            },
        );
    }

    /// 自动档下嗅出来的编码要管到发侧去：对端是中文 Windows 时，
    /// "远端 → 本地"通了之后"本地 → 远端"也得通，两个方向不能一边倒。
    #[test]
    fn what_was_sniffed_on_the_way_in_is_used_on_the_way_out() {
        let mut sync = ClipboardSync::new(VncClipboardCharset::Auto);
        // 嗅之前：中文按 UTF-8 发（自动档的出厂行为）。
        assert!(is_classic(
            sync.local_text("你好".into()),
            "你好".as_bytes()
        ));

        // 对端复制了一份 GBK 的中文过来。
        assert_eq!(sync.on_classic(&[0xC4, 0xE3, 0xBA, 0xC3]), "你好");

        // 之后本地再复制，就该按 GBK 发了。
        assert!(
            is_classic(sync.local_text("好的".into()), &[0xBA, 0xC3, 0xB5, 0xC4]),
            "嗅出对端是 GBK 之后，发侧也该按 GBK",
        );
    }

    /// 点过名的会话不嗅：用户说了是 Big5，就不该因为一份"看着像 GBK"的
    /// 正文把发侧改成 GBK。
    #[test]
    fn a_named_session_never_changes_its_mind() {
        let mut sync = ClipboardSync::new(VncClipboardCharset::Big5);
        sync.on_classic(&[0xC4, 0xE3, 0xBA, 0xC3]);
        assert_eq!(sync.outgoing_charset(), VncClipboardCharset::Big5);
        assert!(is_classic(
            sync.local_text("中文".into()),
            &[0xA4, 0xA4, 0xA4, 0xE5],
        ));
    }

    /// 一条消息是不是"经典正文，内容等于 `expected`"。
    fn is_classic(event: Option<X11Event>, expected: &[u8]) -> bool {
        matches!(event, Some(X11Event::CopyText(bytes)) if bytes == expected)
    }

    #[test]
    fn without_caps_the_classic_path_carries_the_text() {
        let mut sync = ClipboardSync::default();
        let event = sync.local_text("你好".into());
        assert!(
            is_classic(event, "你好".as_bytes()),
            "对端没报 Caps，正文该按经典路径出去",
        );
    }

    #[test]
    fn with_caps_a_local_copy_only_notifies_and_provides_on_request() {
        let mut sync = extended();
        let event = sync.local_text("你好".into());
        assert!(
            matches!(
                event,
                Some(X11Event::Clipboard(ExtendedClipboard::Notify {
                    formats: FORMAT_TEXT
                }))
            ),
            "扩展路径下正文不该跟着 Notify 一起出门，实际 {event:?}",
        );

        let reaction = sync.on_extended(ExtendedClipboard::Request {
            formats: FORMAT_TEXT,
        });
        match reaction.reply {
            Some(X11Event::Clipboard(ExtendedClipboard::Provide { items })) => {
                // 文本块末尾那个 NUL 是惯例（见 [`ClipboardSync::provide`]）。
                assert_eq!(items, vec![(FORMAT_TEXT, "你好\0".as_bytes().to_vec())]);
            }
            other => panic!("对端来要，该 Provide 一份文本，实际 {other:?}"),
        }
    }

    #[test]
    fn a_notify_from_the_remote_is_answered_with_a_request() {
        let mut sync = extended();
        let reaction = sync.on_extended(ExtendedClipboard::Notify {
            formats: FORMAT_TEXT,
        });
        assert!(
            matches!(
                reaction.reply,
                Some(X11Event::Clipboard(ExtendedClipboard::Request {
                    formats: FORMAT_TEXT
                }))
            ),
            "收到 Notify 该去要，实际 {:?}",
            reaction.reply,
        );
        assert!(reaction.clipboard.is_none());

        let reaction = sync.on_extended(ExtendedClipboard::Provide {
            items: vec![(FORMAT_TEXT, "远端复制的中文".as_bytes().to_vec())],
        });
        assert_eq!(reaction.clipboard.as_deref(), Some("远端复制的中文"));
    }

    #[test]
    fn a_peek_is_answered_with_a_notify_only_when_there_is_something_to_offer() {
        let mut sync = extended();
        let reaction = sync.on_extended(ExtendedClipboard::Peek {
            formats: FORMAT_TEXT,
        });
        assert!(reaction.reply.is_none(), "手上没有文本就别报有");

        sync.local_text("你好".into());
        let reaction = sync.on_extended(ExtendedClipboard::Peek {
            formats: FORMAT_TEXT,
        });
        assert!(
            matches!(
                reaction.reply,
                Some(X11Event::Clipboard(ExtendedClipboard::Notify {
                    formats: FORMAT_TEXT
                }))
            ),
            "Peek 只是问有没有，回一声 Notify 就够，实际 {:?}",
            reaction.reply,
        );
    }

    #[test]
    fn the_remote_limit_is_honoured() {
        let mut sync = ClipboardSync::default();
        sync.on_extended(ExtendedClipboard::Caps(Caps {
            formats: FORMAT_TEXT,
            actions: ACTION_CAPS | ACTION_REQUEST | ACTION_NOTIFY | ACTION_PROVIDE,
            limits: vec![(FORMAT_TEXT, 4)],
        }));
        assert!(
            sync.local_text("五个字以上".into()).is_none(),
            "超过对端声明的上限，连 Notify 都不该发",
        );
        let reaction = sync.on_extended(ExtendedClipboard::Request {
            formats: FORMAT_TEXT,
        });
        assert!(reaction.reply.is_none(), "没报过、也没存下来，来要也没有");
    }

    #[test]
    fn a_peer_without_the_text_format_stays_on_the_classic_path() {
        let mut sync = ClipboardSync::default();
        // 认这个扩展，但只收 HTML —— 没有能放我们文本的地方。
        sync.on_extended(ExtendedClipboard::Caps(caps(
            FORMAT_HTML,
            ACTION_CAPS | ACTION_REQUEST | ACTION_NOTIFY | ACTION_PROVIDE,
        )));
        assert!(is_classic(
            sync.local_text("你好".into()),
            "你好".as_bytes()
        ));
    }

    #[test]
    fn a_peer_that_cannot_be_asked_is_not_asked() {
        let mut sync = ClipboardSync::default();
        // 会通知我们，但 Caps 里没报 REQUEST：问了也没人应。
        sync.on_extended(ExtendedClipboard::Caps(caps(
            FORMAT_TEXT,
            ACTION_CAPS | ACTION_NOTIFY | ACTION_PROVIDE,
        )));
        let reaction = sync.on_extended(ExtendedClipboard::Notify {
            formats: FORMAT_TEXT,
        });
        assert!(reaction.reply.is_none(), "对端不认 Request，就别去要");
    }

    #[test]
    fn an_inbound_text_becomes_what_we_offer_back() {
        let mut sync = extended();
        // 对端复制来的文本会写进本地剪贴板 —— 那我们手上这份就该换成它，
        // 否则对端随后再来问，我们给的是上一份、早就不是剪贴板里的内容了。
        assert_eq!(sync.on_classic("经典来的".as_bytes()), "经典来的");
        let reaction = sync.on_extended(ExtendedClipboard::Peek {
            formats: FORMAT_TEXT,
        });
        assert!(
            matches!(
                reaction.reply,
                Some(X11Event::Clipboard(ExtendedClipboard::Notify { .. }))
            ),
            "剪贴板里已经是这份文本，对端来问就该说有",
        );

        sync.on_extended(ExtendedClipboard::Provide {
            items: vec![(FORMAT_TEXT, "扩展来的".as_bytes().to_vec())],
        });
        let reaction = sync.on_extended(ExtendedClipboard::Request {
            formats: FORMAT_TEXT,
        });
        assert!(
            matches!(
                &reaction.reply,
                Some(X11Event::Clipboard(ExtendedClipboard::Provide { items }))
                    if *items == vec![(FORMAT_TEXT, "扩展来的\0".as_bytes().to_vec())]
            ),
            "给出去的得是手上这一份（对端刚给我们的那份），实际 {:?}",
            reaction.reply,
        );
    }

    #[test]
    fn the_own_caps_reply_goes_out_once() {
        let mut sync = extended();
        let again = sync.on_extended(ExtendedClipboard::Caps(caps(
            FORMAT_TEXT,
            ACTION_CAPS | ACTION_NOTIFY | ACTION_PROVIDE,
        )));
        assert!(again.reply.is_none(), "Caps 只回一次");
    }
