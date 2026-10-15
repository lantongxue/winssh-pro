// WebDAV 连接层的纯函数：挂载点、请求路径、href 三套路径的折算与百分号编码。

    use super::*;

    #[test]
    fn mount_comes_from_the_endpoint_path() {
        assert_eq!(mount_of("/dav"), "/dav");
        assert_eq!(mount_of("/dav/"), "/dav");
        assert_eq!(mount_of("/remote.php/dav/files/me/"), "/remote.php/dav/files/me");
        // 站点根就是没有挂载点。
        assert_eq!(mount_of("/"), "");
        assert_eq!(mount_of(""), "");
        // 端点里编码过的中文段，挂载点存的是解码后的原字符。
        assert_eq!(mount_of("/%E7%BD%91%E7%9B%98"), "/网盘");
    }

    #[test]
    fn requests_carry_the_mount_point() {
        // 线上踩过的坑（alist / openlist，端点 `https://host/dav`）：最早一版发请求
        // 时漏拼了挂载点，`PROPFIND` 打到站点根的 `/winssh_backups` 上，服务器回
        // 405，备份列表报错、文件面板进不去目录。面板的 `/` 必须是服务器上的 `/dav`。
        assert_eq!(request_path("/dav", "/"), "dav");
        assert_eq!(request_path("/dav", "/winssh_backups"), "dav/winssh_backups");
        assert_eq!(request_path("/dav", "/a/b.txt"), "dav/a/b.txt");
        assert_eq!(request_path("", "/a/b.txt"), "a/b.txt");
        assert_eq!(request_path("", "/"), "");
    }

    #[test]
    fn request_path_encodes_each_segment() {
        assert_eq!(request_path("/dav", "/备份/a b.txt"), "dav/%E5%A4%87%E4%BB%BD/a%20b.txt");
        // `#` 不编会被当成片段截掉、`?` 会变成查询串、`%` 会被当成转义。
        assert_eq!(request_path("", "/a#1?.txt"), "a%231%3F.txt");
        assert_eq!(request_path("", "/100%.txt"), "100%25.txt");
        // 挂载点本身含中文也一样编。
        assert_eq!(request_path("/网盘", "/x"), "%E7%BD%91%E7%9B%98/x");
    }

    #[test]
    fn href_comes_back_as_panel_path() {
        assert_eq!(panel_of_href("/dav", "/dav/"), "/");
        assert_eq!(panel_of_href("/dav", "/dav"), "/");
        assert_eq!(panel_of_href("/dav", "/dav/winssh_backups/"), "/winssh_backups");
        assert_eq!(
            panel_of_href("/dav", "/dav/%E5%A4%87%E4%BB%BD/a%20b.txt"),
            "/备份/a b.txt"
        );
        // 有的服务器回完整 URL。
        assert_eq!(panel_of_href("/dav", "https://host/dav/a.txt"), "/a.txt");
        assert_eq!(panel_of_href("", "/a/b"), "/a/b");
    }

    #[test]
    fn mount_is_stripped_by_whole_segment() {
        // 挂载点 `/dav` 不能把兄弟目录 `/dav2/x` 减成 `2/x`。
        assert_eq!(panel_of_href("/dav", "/dav2/x"), "/dav2/x");
    }

    #[test]
    fn request_and_href_are_inverse() {
        for panel in ["/", "/a", "/备份/a b.txt", "/x/100%.txt"] {
            let href = format!("/{}", request_path("/dav", panel));
            assert_eq!(panel_of_href("/dav", &href), panel);
        }
    }

    #[test]
    fn home_root_is_relative_to_the_mount() {
        assert_eq!(home_within("/dav", ""), "/");
        assert_eq!(home_within("/dav", "/备份"), "/备份");
        assert_eq!(home_within("/dav", "备份/"), "/备份");
        // 用户把服务器上的完整路径填进根目录：剥掉挂载点，不拼成 `/dav/dav/备份`。
        assert_eq!(home_within("/dav", "/dav/备份"), "/备份");
        assert_eq!(home_within("/dav", "/dav"), "/");
        assert_eq!(home_within("/dav", "/dav2"), "/dav2");
        assert_eq!(home_within("", "/a"), "/a");
    }

    #[test]
    fn percent_decode_handles_chinese_and_spaces() {
        assert_eq!(percent_decode("%E4%B8%AD%E6%96%87.txt"), "中文.txt");
        assert_eq!(percent_decode("a%20b.txt"), "a b.txt");
        assert_eq!(percent_decode("plain.txt"), "plain.txt");
    }

    #[test]
    fn percent_decode_keeps_broken_input() {
        // 一个解不开的 href 不能让整个目录列不出来 —— 宁可原样显示一个带 `%` 的名字。
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
        assert_eq!(percent_decode("%FF%FE"), "%FF%FE");
    }

    #[test]
    fn percent_encode_keeps_unreserved() {
        assert_eq!(percent_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(percent_encode("中文.txt"), "%E4%B8%AD%E6%96%87.txt");
        assert_eq!(percent_encode("a b"), "a%20b");
    }

    fn mismatch(code: u16) -> DavError {
        DavError::Decode(reqwest_dav::DecodeError::StatusMismatched(
            reqwest_dav::StatusMismatchedError {
                response_code: code,
                expected_code: 207,
            },
        ))
    }

    #[test]
    fn not_found_is_only_404() {
        assert!(is_not_found(&mismatch(404)));
        // 405 是"这个地址不接受 WebDAV 方法" —— 端点没指到挂载点时站点根回的
        // 就是它。当成"不存在"的话，备份列表会悄悄变空，用户以为自己没备份过。
        assert!(!is_not_found(&mismatch(405)));
        assert!(!is_not_found(&mismatch(401)));
        assert!(!is_not_found(&mismatch(409)));
    }

    #[test]
    fn method_not_allowed_points_at_the_mount_path() {
        let message = describe(&mismatch(405));
        assert!(message.contains("405"));
        assert_ne!(message, describe(&mismatch(404)));
    }

    #[test]
    fn unauthorized_says_credentials_not_path() {
        assert!(describe(&mismatch(401)).contains("401"));
    }
