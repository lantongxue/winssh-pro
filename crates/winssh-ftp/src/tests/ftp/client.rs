// FTP 连接层的几条纯规则：数据通道连哪儿、错误之后连接还能不能回池、回复怎么说成人话。
//
// 对着真实服务器跑的那几条（`live_*`，平时 `#[ignore]`）跨 ftp + service，留在
// 根包 `src/tests/ftp/client.rs`（那边能拿到 `service::FtpService`）。

    use super::*;

    #[test]
    fn direct_data_channel_uses_control_peer_not_pasv_address() {
        // 服务器在 NAT 后面时 PASV 回的是内网地址，照着连就是超时；
        // 所以直连时一律连控制连接的对端，只取 PASV 的端口。
        let peer: IpAddr = "203.0.113.7".parse().unwrap();
        assert_eq!(data_host(false, Some(peer), "ftp.example.com"), "203.0.113.7");
        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(data_host(false, Some(v6), "ftp.example.com"), "2001:db8::1");
    }

    #[test]
    fn proxied_data_channel_uses_hostname() {
        // 经代理时控制连接的对端是代理本身，连它的端口毫无意义 —— 交主机名给代理去解析。
        let proxy_peer: IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(data_host(true, Some(proxy_peer), "ftp.example.com"), "ftp.example.com");
        assert_eq!(data_host(true, None, "ftp.example.com"), "ftp.example.com");
    }

    #[test]
    fn server_refusal_keeps_connection_reusable() {
        let refusal = FtpError::UnexpectedResponse(suppaftp::types::Response::new(
            Status::FileUnavailable,
            b"550 No such file.\r\n".to_vec(),
        ));
        assert!(reusable_after(&refusal));
    }

    #[test]
    fn broken_connection_is_not_reused() {
        // 超时、断线之后服务器那头可能还欠着一句回复，放回池里下一个人就对不上了。
        assert!(!reusable_after(&FtpError::ConnectionError(
            io::ErrorKind::TimedOut.into()
        )));
        assert!(!reusable_after(&FtpError::SecureError("handshake".into())));
        assert!(!reusable_after(&FtpError::BadResponse));
    }

    #[test]
    fn reply_text_joins_multiline_replies() {
        assert_eq!(reply_text(b"550 Failed to open file.\r\n"), "550 Failed to open file.");
        assert_eq!(
            reply_text(b"530-Login incorrect.\r\n530 Please try again.\r\n"),
            "530-Login incorrect. 530 Please try again."
        );
    }

    #[test]
    fn describe_shows_server_reply_verbatim() {
        let refusal = FtpError::UnexpectedResponse(suppaftp::types::Response::new(
            Status::NotLoggedIn,
            b"530 Login incorrect.\r\n".to_vec(),
        ));
        assert_eq!(describe(&refusal), "530 Login incorrect.");
    }
