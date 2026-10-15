        use super::imp::*;

        /// base64 与 `openssl base64` / `base64` 的标准输出一致。钉住三个
        /// 边界：长度 0/1/2/3 mod 3。
        #[test]
        fn base64_matches_the_standard_alphabet() {
            assert_eq!(base64_encode(b""), "");
            assert_eq!(base64_encode(b"f"), "Zg==");
            assert_eq!(base64_encode(b"fo"), "Zm8=");
            assert_eq!(base64_encode(b"foo"), "Zm9v");
            assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
            assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
            assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        }

        /// install script 里的引号 pairing 不能让 shell 看到 off-by-one —
        /// 万一 exe 路径里有空格或单引号，整段还是要能跑过去。
        #[test]
        fn shell_quote_keeps_apostrophes_inside_single_quotes() {
            assert_eq!(shell_quote_for_script(""), "''");
            assert_eq!(shell_quote_for_script("foo"), "'foo'");
            assert_eq!(shell_quote_for_script("with space"), "'with space'");
            assert_eq!(shell_quote_for_script("don't"), "'don'\\''t'");
            assert_eq!(
                shell_quote_for_script("it's a 'test'"),
                "'it'\\''s a '\\''test'\\'''"
            );
        }

        /// AppleScript 的 escape 集只有 `"` 与 `\`。
        #[test]
        fn applescript_escape_only_touches_quotes_and_backslashes() {
            assert_eq!(applescript_escape("plain"), "plain");
            assert_eq!(applescript_escape(r#"say "hi""#), r#"say \"hi\""#);
            assert_eq!(applescript_escape(r"C:\path"), r"C:\\path");
            // 单引号、$、反引号在 AppleScript 字符串里不需要 escape。
            assert_eq!(applescript_escape("don't $HOME `cmd`"), "don't $HOME `cmd`");
        }
