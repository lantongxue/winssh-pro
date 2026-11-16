    use super::TextEncoding;

    /// UTF-8 的两档：BOM 解的时候去掉，存回去原样带上。
    #[test]
    fn utf8_bom_is_stripped_from_the_text_and_written_back() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("hello 你好".as_bytes());
        assert!(TextEncoding::has_utf8_bom(&bytes));
        assert!(!TextEncoding::has_utf8_bom("hello".as_bytes()));

        let text = TextEncoding::Utf8Bom.decode(&bytes).expect("能解");
        assert_eq!(text, "hello 你好", "BOM 不是正文，不该出现在编辑器里");

        let saved = TextEncoding::Utf8Bom.encode(&text).expect("能编");
        assert_eq!(saved, bytes, "存回远端要带着原来的 BOM");
    }

    /// GBK 的字节按 GBK 解、按 GBK 编回去；同一份字节按 UTF-8 解不动就报错。
    #[test]
    fn gbk_round_trips_chinese_and_utf8_refuses_those_bytes() {
        // “你好”在 GBK 里是 C4 E3 BA C3。
        let bytes = vec![0xC4, 0xE3, 0xBA, 0xC3];
        assert_eq!(TextEncoding::Gbk.decode(&bytes).as_deref(), Some("你好"));
        assert_eq!(TextEncoding::Gbk.encode("你好"), Some(bytes.clone()));
        assert_eq!(
            TextEncoding::Utf8.decode(&bytes),
            None,
            "不是合法 UTF-8 就报错，不拿替换字符糊过去"
        );
    }

    /// 解不动的字节返回 `None` —— 替换字符写回远端会把文件改坏。
    #[test]
    fn undecodable_bytes_fail_instead_of_turning_into_replacement_characters() {
        assert_eq!(TextEncoding::Utf8.decode(&[0xFF, 0xFE, 0x00]), None);
    }

    /// 编不下的字符让保存失败，而不是写一串 `?` 到远端。
    #[test]
    fn characters_the_table_cannot_hold_fail_the_save() {
        // GBK 没有 emoji 的位置。
        assert_eq!(TextEncoding::Gbk.encode("😀"), None);
        // 同一档里能表示的部分不受影响。
        assert!(TextEncoding::Gbk.encode("你好").is_some());
    }
