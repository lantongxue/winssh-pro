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
    fn untrusted_certificate_with_different_pin_is_refused() {
        // 和 SSH 的 KeyChanged 同一条规矩：可能是中间人，不给"继续"。
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
