    use super::{language_for, language_for_shebang};

    #[test]
    fn maps_by_extension() {
        assert_eq!(language_for("main.rs").as_deref(), Some("rust"));
        assert_eq!(language_for("deploy.sh").as_deref(), Some("bash"));
        assert_eq!(language_for("app.py").as_deref(), Some("python"));
        assert_eq!(language_for("server.go").as_deref(), Some("go"));
        assert_eq!(language_for("index.tsx").as_deref(), Some("tsx"));
        assert_eq!(language_for("query.sql").as_deref(), Some("sql"));
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert_eq!(language_for("Config.TOML").as_deref(), Some("toml"));
        assert_eq!(language_for("Data.YAML").as_deref(), Some("yaml"));
    }

    #[test]
    fn maps_special_names() {
        assert_eq!(language_for("Makefile").as_deref(), Some("make"));
        assert_eq!(language_for(".bashrc").as_deref(), Some("bash"));
        assert_eq!(language_for(".zshrc").as_deref(), Some("bash"));
        assert_eq!(language_for("CMakeLists.txt").as_deref(), Some("cmake"));
    }

    #[test]
    fn unknown_falls_back_to_plain() {
        assert_eq!(language_for("README"), None);
        assert_eq!(language_for("notes.unknownext"), None);
        assert_eq!(language_for("archive.tar.gz"), None);
        // 无扩展名、无特例：交给 shebang 兜底，不硬猜。
        assert_eq!(language_for("run"), None);
    }

    #[test]
    fn shebang_detects_interpreters() {
        assert_eq!(language_for_shebang("#!/bin/bash").as_deref(), Some("bash"));
        assert_eq!(
            language_for_shebang("#!/bin/sh -e").as_deref(),
            Some("bash")
        );
        assert_eq!(
            language_for_shebang("#!/usr/bin/env python3").as_deref(),
            Some("python")
        );
        assert_eq!(
            language_for_shebang("#!/usr/bin/env node").as_deref(),
            Some("javascript")
        );
        assert_eq!(language_for_shebang("not a shebang"), None);
        assert_eq!(language_for_shebang("#!/unknown/thing"), None);
    }
