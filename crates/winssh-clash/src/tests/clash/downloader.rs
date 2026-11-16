    use super::*;

    #[test]
    fn bytes_read_short() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(12_897_485), "12.3 MB");
    }

    #[test]
    fn fraction_needs_a_total() {
        let mut state = DownloadState {
            downloaded: 50,
            total: Some(200),
            ..DownloadState::default()
        };
        assert_eq!(state.fraction(), Some(0.25));
        state.total = None;
        assert_eq!(state.fraction(), None);
    }

    #[test]
    fn test_platform_detect() {
        let os = detect_os().expect("os should be supported");
        let arch = detect_arch().expect("arch should be supported");
        assert!(!os.is_empty());
        assert!(!arch.is_empty());
    }

    #[test]
    fn test_asset_name_format() {
        let name = release_asset_name("v1.19.32").expect("asset name should succeed");
        assert!(name.starts_with("mihomo-"));
        assert!(name.contains("v1.19.32"));
        #[cfg(target_os = "macos")]
        {
            assert!(name.starts_with("mihomo-darwin-"));
            assert!(name.ends_with(".gz"));
        }
        #[cfg(target_os = "windows")]
        {
            assert!(name.starts_with("mihomo-windows-"));
            assert!(name.ends_with(".zip"));
        }
    }

    #[test]
    fn test_download_url() {
        let url = download_url("v1.19.32").expect("url should succeed");
        assert!(url.starts_with("https://github.com/MetaCubeX/mihomo/releases/download/v1.19.32/"));
    }

    #[test]
    fn tag_input_gets_a_v_prefix() {
        assert_eq!(normalize_tag("1.19.34").unwrap(), "v1.19.34");
        assert_eq!(normalize_tag(" v1.19.34 ").unwrap(), "v1.19.34");
        // 非数字开头的 tag 原样保留（mihomo 也发 Prerelease-Alpha 这种）。
        assert_eq!(normalize_tag("Prerelease-Alpha").unwrap(), "Prerelease-Alpha");
    }

    #[test]
    fn tag_input_rejects_path_escapes() {
        // tag 会拼进临时文件名；`../` 爬出目录过一次，这里不能放行。
        assert!(normalize_tag("").is_err());
        assert!(normalize_tag("../evil").is_err());
        assert!(normalize_tag("v1.19.34/../../x").is_err());
        assert!(normalize_tag(".hidden").is_err());
        assert!(normalize_tag("v1 19 34").is_err());
    }

    #[test]
    fn auto_source_tries_mirrors_then_direct() {
        let original = "https://github.com/MetaCubeX/mihomo/releases/download/v1.19.34/a.zip";
        let urls = candidate_urls(original, DownloadSource::Auto);
        assert_eq!(urls.len(), MIRRORS.len() + 1);
        assert_eq!(urls[0], format!("{}{original}", MIRRORS[0]));
        assert_eq!(urls.last().unwrap(), original);
    }

    #[test]
    fn direct_source_skips_mirrors() {
        let original = "https://github.com/x/y";
        assert_eq!(candidate_urls(original, DownloadSource::Direct), vec![original.to_string()]);
    }

    #[test]
    fn mirror_source_uses_only_that_mirror() {
        let original = "https://github.com/x/y";
        let urls = candidate_urls(original, DownloadSource::Mirror(2));
        assert_eq!(urls, vec![format!("{}{original}", MIRRORS[2])]);
    }

    #[test]
    fn test_unpack_gz() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir().join("winssh_test_gz");
        let _ = std::fs::create_dir_all(&temp_dir);
        let src_gz = temp_dir.join("test.gz");
        let dst_bin = temp_dir.join("test_out");

        let expected_bytes = b"#!/bin/sh\necho 'hello mihomo'\n";
        {
            let file = std::fs::File::create(&src_gz).unwrap();
            let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            encoder.write_all(expected_bytes).unwrap();
            encoder.finish().unwrap();
        }

        unpack_gz(&src_gz, &dst_bin).expect("unpack_gz should succeed");
        let read_bytes = std::fs::read(&dst_bin).expect("should read output file");
        assert_eq!(read_bytes, expected_bytes);

        let _ = std::fs::remove_file(&src_gz);
        let _ = std::fs::remove_file(&dst_bin);
        let _ = std::fs::remove_dir(&temp_dir);
    }

    #[test]
    fn test_unpack_zip() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir().join("winssh_test_zip");
        let _ = std::fs::create_dir_all(&temp_dir);
        let src_zip = temp_dir.join("test.zip");
        let dst_bin = temp_dir.join("test_out.exe");

        let expected_bytes = b"MZtest_executable_binary";
        {
            let file = std::fs::File::create(&src_zip).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("mihomo-windows-amd64.exe", options).unwrap();
            zip.write_all(expected_bytes).unwrap();
            zip.finish().unwrap();
        }

        unpack_zip(&src_zip, &dst_bin).expect("unpack_zip should succeed");
        let read_bytes = std::fs::read(&dst_bin).expect("should read output file");
        assert_eq!(read_bytes, expected_bytes);

        let _ = std::fs::remove_file(&src_zip);
        let _ = std::fs::remove_file(&dst_bin);
        let _ = std::fs::remove_dir(&temp_dir);
    }

    #[tokio::test]
    #[ignore] // 走真网络（含镜像 API），按需手跑
    async fn test_resolve_latest_tag() {
        let tag = resolve_latest_tag(DownloadSource::Auto)
            .await
            .expect("latest tag should resolve");
        assert!(!tag.is_empty());
        assert!(!tag.contains('/'));
    }

    #[tokio::test]
    #[ignore]
    async fn test_real_download_mihomo() {
        let res = download_mihomo(DownloadRequest::default(), |event| {
            println!("Download event: {event:?}");
        })
        .await;
        assert!(res.is_ok(), "download failed: {:?}", res.err());
        let bin = res.unwrap();
        assert!(bin.is_file());
    }

    #[test]
    #[ignore] // 需要本地已安装/下载好 mihomo 内核，按需手动跑
    fn test_binary_path_finds_downloaded() {
        let path = super::super::runtime::binary_path();
        assert!(
            path.is_ok(),
            "binary_path should locate the binary: {:?}",
            path.err()
        );
        let bin = path.unwrap();
        assert!(bin.is_file());
    }
