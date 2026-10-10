    use super::*;

    /// `sanitize_device_name` 把空白 / 分隔符剥干净；空名字给默认。
    #[test]
    fn device_name_is_stripped_to_yaml_safe_chars() {
        assert_eq!(sanitize_device_name("winssh-tun"), "winssh-tun");
        assert_eq!(sanitize_device_name("utun9"), "utun9");
        assert_eq!(sanitize_device_name("   "), "winssh-tun");
        assert_eq!(sanitize_device_name("../../x"), "....x"); // 点留着，斜杠没了
        assert_eq!(sanitize_device_name("wins sh"), "winssh");
        assert_eq!(sanitize_device_name("wins/h\\i:s"), "winshis");
    }
