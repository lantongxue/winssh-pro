// FTPS 证书信任的几条纯规则：怎么处置一张证书、指纹怎么写、报错怎么说成人话。

    use super::*;

    const FP_A: &str = "SHA256:AA";
    const FP_B: &str = "SHA256:BB";

    #[test]
    fn system_trusted_certificate_never_asks() {
        // 系统证书库认的证书（Let's Encrypt 之类）直接放行 —— 哪怕库里钉过别的指纹：
        // 那多半是服务器从自签名换成了正经证书，或者正经证书续期了。
        assert_eq!(verdict(true, None, FP_A), Verdict::Trusted);
        assert_eq!(verdict(true, Some(FP_B), FP_A), Verdict::Trusted);
    }

    #[test]
    fn untrusted_certificate_without_pin_asks_the_user() {
        assert_eq!(verdict(false, None, FP_A), Verdict::Unknown);
    }

    #[test]
    fn untrusted_certificate_matching_pin_passes() {
        assert_eq!(verdict(false, Some(FP_A), FP_A), Verdict::Pinned);
    }

    #[test]
    fn untrusted_certificate_with_different_pin_is_changed() {
        // 和 SSH 的 KeyChanged 同一条规矩：可能是中间人，也可能是服务器换了证书 ——
        // 不自行放行，捕获新旧指纹交用户定夺（替换 / 拒绝）。
        assert_eq!(verdict(false, Some(FP_B), FP_A), Verdict::Changed);
    }

    #[test]
    fn fingerprint_is_uppercase_colon_hex_sha256() {
        // 和 `openssl x509 -noout -fingerprint -sha256` 同一个写法，用户要拿它去服务器上核对。
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(
            fingerprint(b"abc"),
            "SHA256:BA:78:16:BF:8F:01:CF:EA:41:41:40:DE:5D:AE:22:23:\
             B0:03:61:A3:96:17:7A:9C:B4:10:FF:61:F2:00:15:AD"
        );
    }

    #[test]
    fn key_type_names_common_algorithms() {
        assert_eq!(key_type_of_oid("1.2.840.113549.1.1.1"), "x509-rsa");
        assert_eq!(key_type_of_oid("1.2.840.10045.2.1"), "x509-ecdsa");
        assert_eq!(key_type_of_oid("1.3.101.112"), "x509-ed25519");
        assert_eq!(key_type_of_oid("1.2.3.4"), "x509");
    }

    #[test]
    fn self_signed_wins_over_unknown_issuer() {
        // Windows 对自签名证书报的是"颁发者未知"（SEC_E_UNTRUSTED_ROOT）——
        // 对用户来说"自签名"才是那句说得清的话。
        let unknown = TlsError::InvalidCertificate(CertificateError::UnknownIssuer);
        assert_eq!(problem_of(Some(&unknown), true), CertProblem::SelfSigned);
        assert_eq!(problem_of(Some(&unknown), false), CertProblem::UnknownIssuer);
    }

    #[test]
    fn specific_problems_are_named() {
        let expired = TlsError::InvalidCertificate(CertificateError::Expired);
        assert_eq!(problem_of(Some(&expired), true), CertProblem::Expired);
        let name = TlsError::InvalidCertificate(CertificateError::NotValidForName);
        assert_eq!(problem_of(Some(&name), false), CertProblem::NameMismatch);
        let revoked = TlsError::InvalidCertificate(CertificateError::Revoked);
        assert_eq!(problem_of(Some(&revoked), false), CertProblem::Revoked);
    }

    #[test]
    fn no_system_store_falls_back_to_self_signed_or_other() {
        assert_eq!(problem_of(None, true), CertProblem::SelfSigned);
        assert_eq!(problem_of(None, false), CertProblem::Other);
    }

    #[test]
    fn unparseable_der_still_gets_a_fingerprint() {
        let info = describe(b"not a certificate", "h", 21, None);
        assert_eq!(info.fingerprint, fingerprint(b"not a certificate"));
        assert_eq!(info.key_type, "x509");
        assert!(info.subject.is_empty());
    }

    #[test]
    fn rejected_slot_is_peeked_not_taken() {
        let trust = CertTrust::default();
        *trust.rejected.lock().unwrap() = Some(describe(b"x", "h", 21, None));
        assert!(trust.rejected().is_some());
        assert!(trust.rejected().is_some());
    }

    // 下面几条走真的校验器：DER 是垃圾字节，系统证书库在哪台机器上都不会认它，
    // 所以结果不随环境漂 —— 正好用来验"系统不认"那一层的捕获行为。
    fn garbage_der() -> CertificateDer<'static> {
        CertificateDer::from(b"definitely not a certificate".to_vec())
    }

    fn server_name() -> ServerName<'static> {
        ServerName::try_from("ftp.example.com").unwrap()
    }

    #[test]
    fn changed_certificate_capture_carries_old_fingerprint() {
        // 服务器重装系统后证书换了：被拒槽里除了新证书详情，还要带上库里那份旧
        // 指纹 —— 确认框的"替换还是拒绝"形态靠它并排展示两份指纹。
        let der = garbage_der();
        let trust = CertTrust::pinned(FP_B.to_string());
        let verifier = TrustVerifier::new(trust.clone(), "ftp.example.com", 21);
        let result = verifier.verify_server_cert(&der, &[], &server_name(), &[], UnixTime::now());
        assert!(result.is_err());
        let info = trust.rejected().expect("被拒证书要落进捕获槽");
        assert_eq!(info.fingerprint, fingerprint(&der));
        assert_eq!(info.old_fingerprint.as_deref(), Some(FP_B));
    }

    #[test]
    fn first_sight_capture_has_no_old_fingerprint() {
        // 首次见到（没钉过指纹）：捕获槽里只有新证书，确认框走"首次信任"形态。
        let der = garbage_der();
        let trust = CertTrust::default();
        let verifier = TrustVerifier::new(trust.clone(), "ftp.example.com", 21);
        let result = verifier.verify_server_cert(&der, &[], &server_name(), &[], UnixTime::now());
        assert!(result.is_err());
        let info = trust.rejected().expect("被拒证书要落进捕获槽");
        assert_eq!(info.old_fingerprint, None);
    }

    #[test]
    fn pinned_certificate_passes_without_system_trust() {
        // 系统不认但指纹与钉住的相同：放行，捕获槽不动 —— 重装后用户点过"替换"，
        // 下一次连走的就是这条路。
        let der = garbage_der();
        let trust = CertTrust::pinned(fingerprint(&der));
        let verifier = TrustVerifier::new(trust.clone(), "ftp.example.com", 21);
        let result = verifier.verify_server_cert(&der, &[], &server_name(), &[], UnixTime::now());
        assert!(result.is_ok());
        assert!(trust.rejected().is_none());
    }

    #[test]
    fn key_info_carries_both_fingerprints() {
        // 折成信任库那一行时新旧指纹都带上：save 落库用的是新指纹，旧指纹供展示。
        let mut info = describe(b"x", "ftp.example.com", 21, None);
        info.old_fingerprint = Some(FP_B.to_string());
        let row = info.key_info();
        assert_eq!(row.hostname, "ftp.example.com");
        assert_eq!(row.port, 21);
        assert_eq!(row.fingerprint, info.fingerprint);
        assert_eq!(row.old_fingerprint.as_deref(), Some(FP_B));
    }
