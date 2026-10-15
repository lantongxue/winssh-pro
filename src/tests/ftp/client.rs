// FTP 连接层**对着真实服务器**跑的几条，平时 `#[ignore]`，要手动验证时解开：
//
//     cargo test ftp::client_tests::live -- --ignored --nocapture
//
// 默认那台是 kali 在一台 Windows 上搭的 FileZilla Server（见下面几个常量）。
// 它要求先 `AUTH TLS`、而且用的是自签名证书 —— 所以"明文连不上""默认校验
// 下证书被拒"都是预期结果，不是 bug。
//
// 这几条跨 ftp + service（要调 `crate::service::FtpService`），所以留在根包；
// 连接层那些用私有项的纯规则单测在 `winssh-ftp` 里（`crates/winssh-ftp/src/tests/ftp/client.rs`）。

    use super::*;
    use crate::ftp::tls::CertTrust;

    /// kali 测试用的 FileZilla Server。
    fn live_host(security: FtpSecurity) -> crate::host::Host {
        crate::host::Host {
            id: "live-ftp".into(),
            name: "live-ftp".into(),
            kind: crate::host::HostKind::Ftp,
            user: "kali".into(),
            hostname: "172.24.134.83".into(),
            port: security.default_port(),
            platform: String::new().into(),
            auth_method: crate::host::AuthMethod::Password,
            password: "1".into(),
            key_content: String::new().into(),
            key_passphrase: String::new().into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: crate::ftp::FtpSettings { security },
            webdav: Default::default(),
            shell_integration: false,
        }
    }

    /// 首次连：自签名证书被拒、详情落进 `rejected`。照确认框那条路钉住指纹再连。
    async fn live_connect(host: &crate::host::Host) -> crate::service::FtpService {
        let first = CertTrust::default();
        match crate::service::FtpService::connect(host, first.clone()).await {
            Ok(service) => return service,
            Err(error) => println!("首次：{error:#}"),
        }
        let info = first.rejected().expect("被拒的证书应当记下来，确认框靠它");
        println!(
            "证书：{} / 颁发者 {} / {}～{} / {:?}\n指纹：{}",
            info.subject, info.issuer, info.not_before, info.not_after, info.problem, info.fingerprint
        );
        crate::service::FtpService::connect(host, CertTrust::pinned(info.fingerprint))
            .await
            .expect("钉住指纹之后应当连得上")
    }

    /// 明文连一台"要求先 AUTH TLS"的服务器：应当报服务器那句 `503 Use AUTH first.`。
    #[test]
    #[ignore = "对着真实服务器，手动跑"]
    fn live_plain_is_refused_by_the_server() {
        crate::db::runtime().block_on(async {
            match crate::service::FtpService::connect(
                &live_host(FtpSecurity::Plain),
                CertTrust::default(),
            )
            .await
            {
                Ok(_) => panic!("这台服务器不接受明文登录"),
                Err(error) => {
                    println!("明文：{error:#}");
                    assert!(format!("{error:#}").contains("AUTH"), "{error:#}");
                }
            }
        });
    }

    /// 显式 FTPS 全程：确认证书 → 登录 → 列目录（数据通道要恢复控制连接的 TLS 会话，
    /// FileZilla Server 不恢复就回 `425 TLS session of data connection not resumed`）。
    #[test]
    #[ignore = "对着真实服务器，手动跑"]
    fn live_explicit_ftps_lists_after_pinning() {
        crate::db::runtime().block_on(async {
            let service = live_connect(&live_host(FtpSecurity::Explicit)).await;
            let home = service.home().await.unwrap();
            println!("登录目录：{home}");
            let entries = service.list(&home).await.expect("列目录");
            println!("{} 项", entries.len());
            for entry in entries.iter().take(10) {
                println!("  {:?} {:>10} {:?} {:?}", entry.kind, entry.size, entry.mode, entry.name.as_ref());
            }
            // 再列两次：连接池里那条连接回池再借出，数据通道每次都要重新恢复会话。
            for _ in 0..2 {
                service.list(&home).await.expect("再列一次");
            }
        });
    }

    /// 一来一回：写一个小文件、读回来比对、改名、删掉。走的都是浏览道。
    #[test]
    #[ignore = "对着真实服务器，手动跑"]
    fn live_explicit_ftps_round_trips_a_file() {
        crate::db::runtime().block_on(async {
            let service = live_connect(&live_host(FtpSecurity::Explicit)).await;
            let home = service.home().await.unwrap();
            let name = crate::sftp::path::join(&home, "winssh-live-test.txt");
            let renamed = crate::sftp::path::join(&home, "winssh-live-test-renamed.txt");
            let body = "你好，FTPS\nline 2\n";

            service.write_text(&name, body).await.expect("写");
            assert_eq!(service.read_text(&name).await.expect("读"), body);
            assert!(service.exists(&name).await.unwrap());
            service.rename(&name, &renamed).await.expect("改名");
            assert!(!service.exists(&name).await.unwrap());
            let entry = service
                .list(&home)
                .await
                .unwrap()
                .into_iter()
                .find(|entry| entry.path.as_ref() == renamed)
                .expect("改名后的文件在列表里");
            assert_eq!(entry.size, body.len() as u64);
            service.remove(&entry).await.expect("删");
            assert!(!service.exists(&renamed).await.unwrap());
        });
    }
