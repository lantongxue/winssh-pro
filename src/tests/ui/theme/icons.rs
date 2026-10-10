    use super::{AppIcon, OsIcon};
    use crate::assets::Assets;
    use gpui_kit::AssetSource as _;
    use gpui_kit::component::IconNamed as _;

    /// 图标的路径写在这个文件里，字节内嵌在 `crate::assets` 里，两处对不上
    /// 就是画一片空白 —— 编译器不管，界面上也不报错，只是那一格没了。
    #[test]
    fn every_icon_path_resolves_to_bytes() {
        let paths = [
            AppIcon::Activity.path(),
            AppIcon::Archive.path(),
            AppIcon::Cable.path(),
            AppIcon::ClipboardPaste.path(),
            AppIcon::Columns2.path(),
            AppIcon::Rows2.path(),
            AppIcon::Square.path(),
            AppIcon::CodeXml.path(),
            AppIcon::Download.path(),
            AppIcon::FileCode.path(),
            AppIcon::FilePlus.path(),
            AppIcon::FolderPlus.path(),
            AppIcon::FolderSync.path(),
            AppIcon::History.path(),
            AppIcon::Home.path(),
            AppIcon::Key.path(),
            AppIcon::KeyRound.path(),
            AppIcon::Keyboard.path(),
            AppIcon::Laptop.path(),
            AppIcon::Monitor.path(),
            AppIcon::Pencil.path(),
            AppIcon::RefreshCw.path(),
            AppIcon::Server.path(),
            AppIcon::ShieldCheck.path(),
            AppIcon::Sparkles.path(),
            AppIcon::Squircle.path(),
            AppIcon::Terminal.path(),
            AppIcon::Trash.path(),
            AppIcon::Upload.path(),
            AppIcon::Zap.path(),
            AppIcon::Logo.path(),
            AppIcon::LogoSlim.path(),
            OsIcon::Alpine.path(),
            OsIcon::Arch.path(),
            OsIcon::CentOs.path(),
            OsIcon::Debian.path(),
            OsIcon::MacOs.path(),
            OsIcon::RedHat.path(),
            OsIcon::Rocky.path(),
            OsIcon::Ubuntu.path(),
            OsIcon::Windows.path(),
            OsIcon::Linux.path(),
        ];

        for path in paths {
            let bytes = Assets
                .load(&path)
                .unwrap_or_else(|error| panic!("{path} 读不出来：{error}"))
                .unwrap_or_else(|| panic!("{path} 没有登记进 crate::assets::OWNED"));
            assert!(
                bytes.starts_with(b"<svg"),
                "{path} 不像一个 svg：{:?}",
                &bytes[..bytes.len().min(16)]
            );
        }
    }

    #[test]
    fn platform_marks_match_versions_and_legacy_spellings() {
        // 现在表单里那九档。
        assert_eq!(OsIcon::from_platform("Ubuntu"), OsIcon::Ubuntu);
        assert_eq!(OsIcon::from_platform("macOS"), OsIcon::MacOs);
        assert_eq!(OsIcon::from_platform("Windows OpenSSH"), OsIcon::Windows);
        // 带版本号，以及老库里一格写好几家的写法 —— 认头一个。
        assert_eq!(OsIcon::from_platform("Ubuntu 24.04"), OsIcon::Ubuntu);
        assert_eq!(
            OsIcon::from_platform("CentOS / RedHat / Rocky"),
            OsIcon::CentOs
        );
        assert_eq!(OsIcon::from_platform("macOS Darwin"), OsIcon::MacOs);
        // 认不出来的落到 Tux，不是空白。
        assert_eq!(OsIcon::from_platform(""), OsIcon::Linux);
        assert_eq!(OsIcon::from_platform("FreeBSD 14"), OsIcon::Linux);
    }
