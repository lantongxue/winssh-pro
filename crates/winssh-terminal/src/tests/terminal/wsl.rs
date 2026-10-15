    use super::*;

    #[test]
    fn utf8_output_yields_one_distro_per_line() {
        let distros = parse_distros(b"Ubuntu-22.04\r\ndebian\r\n\r\n");
        assert_eq!(distros, ["Ubuntu-22.04", "debian"]);
    }

    /// `wsl.exe` 往管道里写的其实是 UTF-16LE。这条就是 "Ubuntu-22.04\r\n"
    /// 的 UTF-16LE 字节加上 BOM，解出来得是那个名字而不是一串 NUL 乱码。
    #[test]
    fn utf16le_output_with_bom_is_decoded() {
        let bytes: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("Ubuntu-22.04\r\n".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(parse_distros(&bytes), ["Ubuntu-22.04"]);
    }

    /// 老一点的 wsl.exe 不带 BOM。NUL 的密度是判据：UTF-8 文本里不该有
    /// NUL，UTF-16 的 ASCII 段每两个字节就有一个。
    #[test]
    fn utf16le_output_without_bom_is_decoded() {
        let bytes: Vec<u8> = "debian\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(parse_distros(&bytes), ["debian"]);
    }

    /// 空输出、只有空行：一个发行版都不报，也不许崩。
    #[test]
    fn empty_output_lists_nothing() {
        assert!(parse_distros(b"").is_empty());
        assert!(parse_distros(b"\r\n\r\n").is_empty());
    }

    /// 发行版名前后可能带空白（`--quiet` 的输出并不总是干净），得剥掉。
    #[test]
    fn names_are_trimmed() {
        let distros = parse_distros(b"  openSUSE-Leap-15.6  \r\n");
        assert_eq!(distros, ["openSUSE-Leap-15.6"]);
    }
