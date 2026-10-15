// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

    use super::*;
    use crate::trusted_host::HostKeyInfo;

    fn sample_info(hostname: &str) -> HostKeyInfo {
        HostKeyInfo {
            hostname: hostname.into(),
            port: 22,
            key_type: "ssh-ed25519".into(),
            fingerprint: "SHA256:aaaabbbbccccdddd".into(),
            old_fingerprint: None,
        }
    }

    #[test]
    fn save_and_query_round_trip() {
        let svc = db::runtime()
            .block_on(TrustedHostService::in_memory())
            .unwrap();
        let info = sample_info("10.0.0.1");

        db::runtime().block_on(async {
            svc.save(&info).await.unwrap();
            let fp = svc.fingerprint_for("10.0.0.1", 22).await.unwrap();
            assert_eq!(fp.as_deref(), Some("SHA256:aaaabbbbccccdddd"));
        });
    }

    #[test]
    fn unknown_host_returns_none() {
        let svc = db::runtime()
            .block_on(TrustedHostService::in_memory())
            .unwrap();
        db::runtime().block_on(async {
            let fp = svc.fingerprint_for("192.168.99.99", 22).await.unwrap();
            assert!(fp.is_none());
        });
    }

    #[test]
    fn different_port_is_different_record() {
        let svc = db::runtime()
            .block_on(TrustedHostService::in_memory())
            .unwrap();
        let info = sample_info("10.0.0.2");
        db::runtime().block_on(async {
            svc.save(&info).await.unwrap();
            let fp22 = svc.fingerprint_for("10.0.0.2", 22).await.unwrap();
            let fp2222 = svc.fingerprint_for("10.0.0.2", 2222).await.unwrap();
            assert!(fp22.is_some());
            assert!(fp2222.is_none());
        });
    }

    #[test]
    fn save_twice_updates_fingerprint() {
        let svc = db::runtime()
            .block_on(TrustedHostService::in_memory())
            .unwrap();
        db::runtime().block_on(async {
            svc.save(&sample_info("10.0.0.3")).await.unwrap();
            let mut updated = sample_info("10.0.0.3");
            updated.fingerprint = "SHA256:newkey".into();
            svc.save(&updated).await.unwrap();
            let fp = svc.fingerprint_for("10.0.0.3", 22).await.unwrap();
            assert_eq!(fp.as_deref(), Some("SHA256:newkey"));
        });
    }

    #[test]
    fn delete_removes_record() {
        let svc = db::runtime()
            .block_on(TrustedHostService::in_memory())
            .unwrap();
        db::runtime().block_on(async {
            svc.save(&sample_info("10.0.0.4")).await.unwrap();
            let all = svc.load_all().await.unwrap();
            let id = all[0].id.to_string();
            let deleted = svc.delete(&id).await.unwrap();
            assert!(deleted);
            let fp = svc.fingerprint_for("10.0.0.4", 22).await.unwrap();
            assert!(fp.is_none());
        });
    }