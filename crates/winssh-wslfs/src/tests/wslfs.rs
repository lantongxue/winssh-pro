//! 真机上的 WSL 冒烟测试。
//!
//! 环境敏感（机器上得真装着 WSL、发行版得指定），按项目规矩标 `#[ignore]`，
//! 且不替人挑发行版 —— `WSL_TEST_DISTRO=Ubuntu-24.04 cargo test -p winssh-wslfs
//! -- --ignored` 才跑。跑的时候验三件事：家目录问得出来、共享列得动、
//! `find` 把属主补回来了。

#![cfg(windows)]

use winssh_sftp::{FileEntry, FileKind};

use super::WslFs;

#[test]
#[ignore = "需要本机装有 WSL，并用 WSL_TEST_DISTRO 指定发行版"]
fn live_lists_the_requested_distro_root() {
    let Ok(distro) = std::env::var("WSL_TEST_DISTRO") else {
        return;
    };
    let fs = WslFs::new(distro);
    let home = winssh_db::runtime()
        .block_on(fs.home())
        .expect("家目录问得出来");
    assert!(home.starts_with('/'), "家目录应该是 Linux 绝对路径：{home}");

    let entries = winssh_db::runtime()
        .block_on(fs.list("/"))
        .expect("根目录列得动");
    assert!(
        entries.iter().any(|entry| entry.name.as_str() == "etc"),
        "根目录里应该有 etc"
    );
    assert!(
        entries.iter().any(|entry| entry.owner.is_some()),
        "find 的属主没补回来（退回 Windows 侧属性了）"
    );
}

/// 在 `/tmp` 下把建、写、读、chmod、改名、删走一遍。名字里带空格，顺手
/// 验一遍共享路径与 `find` 的引号处理。
#[test]
#[ignore = "需要本机装有 WSL，并用 WSL_TEST_DISTRO 指定发行版"]
fn live_round_trip_under_tmp() {
    let Ok(distro) = std::env::var("WSL_TEST_DISTRO") else {
        return;
    };
    let fs = WslFs::new(distro.clone());
    let base = format!("/tmp/winssh wslfs test {}", std::process::id());
    let dir_entry = |path: &str| FileEntry {
        name: String::new().into(),
        path: path.to_owned().into(),
        kind: FileKind::Dir,
        size: 0,
        mode: None,
        uid: None,
        owner: None,
        modified: None,
    };

    winssh_db::runtime().block_on(async {
        // 上一次失败可能留了个壳，先清掉；没有也正常。
        let _ = fs.remove(&dir_entry(&base)).await;

        fs.create_dir(&base).await.expect("建目录");
        let file = format!("{base}/hello.txt");
        fs.create_file(&file).await.expect("建文件");
        fs.write_bytes(&file, b"hello wsl".to_vec())
            .await
            .expect("写文件");
        assert_eq!(fs.read_bytes(&file).await.expect("读文件"), b"hello wsl");
        assert_eq!(fs.file_len(&file).await.expect("文件大小"), 9);

        fs.set_mode(&file, 0o640).await.expect("chmod");
        let entries = fs.list(&base).await.expect("列目录");
        let entry = entries
            .iter()
            .find(|entry| entry.name.as_str() == "hello.txt")
            .expect("刚建的文件在列表里");
        assert_eq!(entry.mode, Some(0o640), "chmod 之后的 mode 应该读回来");

        let renamed = format!("{base}/renamed.txt");
        fs.rename(&file, &renamed).await.expect("改名");
        assert!(!fs.exists(&file).await.expect("旧名字"));
        assert!(fs.exists(&renamed).await.expect("新名字"));

        // 单文件删除
        let single = format!("{base}/single.txt");
        fs.create_file(&single).await.expect("建第二个文件");
        fs.remove(&FileEntry {
            path: single.clone().into(),
            kind: FileKind::File,
            ..dir_entry(&single)
        })
        .await
        .expect("删文件");
        assert!(!fs.exists(&single).await.expect("单文件没了"));

        // 软链接只删链接本身：让它指向 /etc/hostname，递归删掉 base 之后
        // 目标还得在。`find` 的 `%y` 应该把它报成 `l`，Windows 那侧不一定。
        let link = format!("{base}/hostname-link");
        let status = std::process::Command::new("wsl.exe")
            .args(["-d", &distro, "-e", "ln", "-s", "/etc/hostname", &link])
            .status()
            .expect("起 wsl.exe 建软链接");
        assert!(status.success(), "ln -s 没成功");
        let entries = fs.list(&base).await.expect("列带软链接的目录");
        let entry = entries
            .iter()
            .find(|entry| entry.name.as_str() == "hostname-link")
            .expect("软链接在列表里");
        assert_eq!(entry.kind, FileKind::Symlink, "find 应该报 l");

        // 目录删除要连子树一起；子目录里再放一个文件。
        let nested = format!("{base}/sub");
        fs.create_dir(&nested).await.expect("建子目录");
        fs.write_bytes(&format!("{nested}/deep.txt"), b"deep".to_vec())
            .await
            .expect("写子目录里的文件");
        fs.remove(&dir_entry(&base)).await.expect("递归删目录");
        assert!(!fs.exists(&base).await.expect("目录没了"));
        assert!(!fs.exists(&renamed).await.expect("目录里的文件也一起没了"));
        assert!(
            fs.exists("/etc/hostname").await.expect("目标还在"),
            "删软链接不该把目标删掉"
        );
    });
}
