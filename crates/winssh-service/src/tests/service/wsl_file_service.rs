//! 真机上的 WSL 传输冒烟测试。
//!
//! 与 `winssh-wslfs` 那两个 `#[ignore]` 用例同一个口径：机器上得真装着 WSL，
//! 用 `WSL_TEST_DISTRO` 指定发行版才跑，在 `/tmp` 与系统临时目录里搭东西、
//! 跑完清掉。验的是面板真正要的那条路 —— 服务摊平目录树、分块搬字节、
//! 进度事件最后落成 `Done { error: None }`。

#![cfg(windows)]

use std::time::Duration;

use tokio::sync::mpsc;

use super::*;
use crate::sftp::{FileKind, TransferDirection};
use crate::sftp_service::TransferEvent;

fn distro() -> Option<String> {
    std::env::var("WSL_TEST_DISTRO").ok()
}

fn dir_entry(path: &str) -> FileEntry {
    FileEntry {
        name: String::new().into(),
        path: path.to_owned().into(),
        kind: FileKind::Dir,
        size: 0,
        mode: None,
        uid: None,
        owner: None,
        modified: None,
    }
}

/// 等一条传输落定；超时直接失败，免得测试挂住。
async fn wait_done(rx: &mut mpsc::UnboundedReceiver<TransferEvent>) -> Option<String> {
    let done = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(event) = rx.recv().await {
            if let TransferEvent::Done { error, .. } = event {
                return error;
            }
        }
        Some("进度通道提前关了".to_owned())
    })
    .await;
    match done {
        Ok(error) => error,
        Err(_) => Some("传输超时".to_owned()),
    }
}

#[test]
#[ignore = "需要本机装有 WSL，并用 WSL_TEST_DISTRO 指定发行版"]
fn live_upload_then_download_round_trip() {
    let Some(distro) = distro() else {
        return;
    };
    let service = WslFileService::new(distro.clone());
    let cleanup = winssh_wslfs::WslFs::new(distro);

    let local_root =
        std::env::temp_dir().join(format!("winssh-wsl-transfer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&local_root);
    let source = local_root.join("pkg");
    std::fs::create_dir_all(source.join("sub")).expect("建本地源目录");
    std::fs::write(source.join("a.txt"), b"alpha").expect("写 a.txt");
    std::fs::write(source.join("sub/b.txt"), b"beta").expect("写 b.txt");

    let remote_dir = format!("/tmp/winssh-wsl-transfer-{}", std::process::id());
    let downloaded = local_root.join("downloaded");

    winssh_db::runtime().block_on(async {
        let _ = cleanup.remove(&dir_entry(&remote_dir)).await;

        // 上传：<remote_dir>/pkg/{a.txt, sub/b.txt}
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = service.spawn_upload(
            TransferJob {
                id: 1,
                direction: TransferDirection::Upload,
                local: source.clone(),
                remote: remote_dir.clone(),
            },
            tx,
        );
        let error = wait_done(&mut rx).await;
        assert_eq!(error, None, "上传失败");
        assert!(handle.is_finished());

        let entries = service
            .list(&format!("{remote_dir}/pkg"))
            .await
            .expect("列上传结果");
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert!(names.contains(&"a.txt"), "上传结果缺 a.txt：{names:?}");
        assert!(names.contains(&"sub"), "上传结果缺 sub：{names:?}");

        // 先下载一遍，落一套本地文件；后面再下一次时就是要顶掉它们。
        let (tx, mut rx) = mpsc::unbounded_channel();
        service.spawn_download(
            TransferJob {
                id: 2,
                direction: TransferDirection::Download,
                local: downloaded.clone(),
                remote: format!("{remote_dir}/pkg"),
            },
            tx,
        );
        let error = wait_done(&mut rx).await;
        assert_eq!(error, None, "下载失败");
        assert_eq!(
            std::fs::read(downloaded.join("a.txt")).expect("读回 a.txt"),
            b"alpha"
        );

        // 改内容再传一次：远端已有同名文件，`replace_remote` 要顶掉旧的。
        std::fs::write(source.join("a.txt"), b"alpha2").expect("改本地 a.txt");
        let (tx, mut rx) = mpsc::unbounded_channel();
        service.spawn_upload(
            TransferJob {
                id: 3,
                direction: TransferDirection::Upload,
                local: source.clone(),
                remote: remote_dir.clone(),
            },
            tx,
        );
        let error = wait_done(&mut rx).await;
        assert_eq!(error, None, "二次上传失败");
        assert_eq!(
            service
                .read_bytes(&format!("{remote_dir}/pkg/a.txt"))
                .await
                .expect("读远端 a.txt"),
            b"alpha2"
        );

        // 再下载一次，本地那套旧文件也要被顶掉。
        let (tx, mut rx) = mpsc::unbounded_channel();
        service.spawn_download(
            TransferJob {
                id: 4,
                direction: TransferDirection::Download,
                local: downloaded.clone(),
                remote: format!("{remote_dir}/pkg"),
            },
            tx,
        );
        let error = wait_done(&mut rx).await;
        assert_eq!(error, None, "二次下载失败");

        let _ = cleanup.remove(&dir_entry(&remote_dir)).await;
    });

    assert_eq!(
        std::fs::read(downloaded.join("a.txt")).expect("读回 a.txt"),
        b"alpha2"
    );
    assert_eq!(
        std::fs::read(downloaded.join("sub/b.txt")).expect("读回 b.txt"),
        b"beta"
    );

    let _ = std::fs::remove_dir_all(&local_root);
}
