    use super::*;

    #[test]
    fn normalize_collapses_noise() {
        assert_eq!(normalize("/var//log/"), "/var/log");
        assert_eq!(normalize("/a/./b/../c"), "/a/c");
        assert_eq!(normalize("/.."), "/");
        assert_eq!(normalize("/../../etc"), "/etc");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("."), ".");
        assert_eq!(normalize("a/../b"), "b");
    }

    #[test]
    fn parent_stops_at_root() {
        assert_eq!(parent("/"), "/");
        assert_eq!(parent("/root"), "/");
        assert_eq!(parent("/var/log"), "/var");
        assert_eq!(parent("/a/b/"), "/a");
    }

    #[test]
    fn join_never_doubles_the_separator() {
        assert_eq!(join("/var", "log"), "/var/log");
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(join("/a/", "../b"), "/b");
        assert_eq!(join("", "b"), "b");
    }

    #[test]
    fn file_name_takes_the_last_segment() {
        assert_eq!(file_name("/var/log/syslog"), "syslog");
        assert_eq!(file_name("/var/log/"), "log");
        assert_eq!(file_name("/"), "");
        assert_eq!(file_name("syslog"), "syslog");
    }

    #[test]
    fn tilde_needs_a_known_home() {
        assert_eq!(expand_tilde("~", Some("/home/kali")), "/home/kali");
        assert_eq!(expand_tilde("~/a", Some("/home/kali")), "/home/kali/a");
        assert_eq!(expand_tilde("~", None), "~");
        assert_eq!(expand_tilde("/etc", Some("/home/kali")), "/etc");
    }

    #[test]
    fn breadcrumbs_build_clickable_prefixes() {
        assert_eq!(
            breadcrumbs("/var/log"),
            vec![
                ("var".into(), "/var".into()),
                ("log".into(), "/var/log".into())
            ]
        );
        assert!(breadcrumbs("/").is_empty());
    }

    #[test]
    fn hidden_means_a_leading_dot() {
        assert!(is_hidden(".bashrc"));
        assert!(!is_hidden(".gitignore/x")); // 只看名字本身，带斜杠说明不是名字
        assert!(!is_hidden("readme"));
    }