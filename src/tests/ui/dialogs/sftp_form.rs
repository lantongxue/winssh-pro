    use super::validate_name;

    #[test]
    fn a_name_is_one_entry_within_one_directory() {
        assert_eq!(validate_name("  app.conf  ").unwrap(), "app.conf");
        assert_eq!(validate_name("app.tar.gz").unwrap(), "app.tar.gz");
        assert_eq!(validate_name(".bashrc").unwrap(), ".bashrc");
        assert!(validate_name("   ").is_err(), "空名字不该发去远端");
        assert!(validate_name("a/b").is_err(), "带斜杠会跑到别的目录去");
        assert!(validate_name(".").is_err());
        assert!(validate_name("..").is_err());
    }
