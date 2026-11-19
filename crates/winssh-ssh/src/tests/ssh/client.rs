// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

    use super::*;
    use russh::keys::{Algorithm, PrivateKey};
    use winssh_domain::trusted_host::KeyCapture;

    /// 生成一把真的 ed25519 公钥：指纹与算法名都从钥匙本身算出来，
    /// 与 keygen 同一条路，不手造第三方类型。
    fn random_key() -> PublicKeyOrCertificate {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        key.public_key().clone().into()
    }

    fn handler(trust: TrustPolicy) -> SshHandler {
        SshHandler::new(
            ForwardRegistry::default(),
            ("10.0.0.1".to_string(), 22),
            trust,
        )
    }

    fn check(handler: &mut SshHandler, key: &PublicKeyOrCertificate) -> Result<bool, russh::Error> {
        winssh_db::runtime().block_on(handler.check_server_key(key))
    }

    #[test]
    fn verify_matching_key_passes_without_capture() {
        let key = random_key();
        let capture = KeyCapture::default();
        let mut h = handler(TrustPolicy::Verify {
            expected: SshHandler::fingerprint_of(&key),
            capture: capture.clone(),
        });
        assert!(check(&mut h, &key).unwrap());
        // 比对通过就不该往槽里写东西：写了会让调用方误弹确认框。
        assert!(capture.lock().unwrap().is_none());
    }

    #[test]
    fn verify_changed_key_captures_old_and_new_fingerprints() {
        // 线上踩过的坑：服务器重装系统后主机密钥变更，旧实现只报 KeyChanged
        // 不捕获新密钥，界面拿不到新指纹，用户没有任何替换信任的入口 ——
        // 那台机器从此连不上。这条测试守"失配必须把新旧指纹都捕获到"。
        let old = random_key();
        let new = random_key();
        let capture = KeyCapture::default();
        let mut h = handler(TrustPolicy::Verify {
            expected: SshHandler::fingerprint_of(&old),
            capture: capture.clone(),
        });
        let err = check(&mut h, &new).unwrap_err();
        assert!(matches!(err, russh::Error::KeyChanged { .. }));
        let info = capture.lock().unwrap().take().expect("新密钥应被捕获");
        assert_eq!(info.fingerprint, SshHandler::fingerprint_of(&new));
        assert_eq!(
            info.old_fingerprint.as_deref(),
            Some(SshHandler::fingerprint_of(&old).as_str()),
        );
        assert_eq!(info.hostname, "10.0.0.1");
        assert_eq!(info.port, 22);
        assert_eq!(info.key_type, "ssh-ed25519");
    }

    #[test]
    fn capture_and_abort_records_key_without_old_fingerprint() {
        let key = random_key();
        let capture = KeyCapture::default();
        let mut h = handler(TrustPolicy::CaptureAndAbort {
            capture: capture.clone(),
        });
        let err = check(&mut h, &key).unwrap_err();
        assert!(matches!(err, russh::Error::UnknownKey));
        let info = capture.lock().unwrap().take().unwrap();
        assert_eq!(info.fingerprint, SshHandler::fingerprint_of(&key));
        // 首次连接没有旧指纹 —— 弹窗靠它区分"信任"与"替换"两种形态。
        assert!(info.old_fingerprint.is_none());
    }

    #[test]
    fn trust_once_accepts_any_key() {
        let key = random_key();
        let mut h = handler(TrustPolicy::TrustOnce);
        assert!(check(&mut h, &key).unwrap());
    }
