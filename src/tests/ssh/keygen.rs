    use super::*;
    use crate::ssh::decode_private_key;

    /// 生成出来的东西必须是我们自己解得开的 —— 这一条是这个模块的全部意义：
    /// 存进保险箱的私钥，下一次开会话时走的就是 `decode_private_key` 那条路。
    ///
    /// RSA 4096 不在这条里跑：生成一次要好几秒，够慢的那一档单独一条测试，
    /// 而"生成的能不能解开"三档是同一套代码路径。
    #[test]
    fn a_generated_key_decodes_back() {
        for algorithm in [KeyAlgorithm::Ed25519, KeyAlgorithm::EcdsaP384] {
            let generated = generate(algorithm, "winssh-pro@test", "").unwrap();
            assert!(
                generated
                    .private_key
                    .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----")
            );
            assert!(generated.fingerprint.starts_with("SHA256:"));
            assert!(
                generated.public_key.ends_with(" winssh-pro@test"),
                "注释没写进公钥行：{}",
                generated.public_key
            );

            decode_private_key(&generated.private_key, "").unwrap_or_else(|error| {
                panic!("{} 生成的私钥解不开：{error:#}", algorithm.badge())
            });
        }
    }

    /// 给了口令就得真的加密：不带口令解不开，带对口令解得开。
    #[test]
    fn a_passphrase_actually_encrypts_the_key() {
        let generated = generate(KeyAlgorithm::Ed25519, "", "hunter2").unwrap();

        assert!(
            decode_private_key(&generated.private_key, "").is_err(),
            "加密过的私钥不该在没有口令时解得开"
        );
        assert!(decode_private_key(&generated.private_key, "not-hunter2").is_err());
        decode_private_key(&generated.private_key, "hunter2").expect("口令对了就该解得开");
    }

    /// 换行一律 LF。Windows 上写成 CRLF 的话，这份私钥贴到 Linux 上就废了。
    #[test]
    fn private_keys_use_lf_line_endings() {
        let generated = generate(KeyAlgorithm::Ed25519, "", "").unwrap();
        assert!(!generated.private_key.contains('\r'));
    }

    /// 两次生成不会是同一把钥匙。
    #[test]
    fn two_keys_are_not_the_same_key() {
        let first = generate(KeyAlgorithm::Ed25519, "", "").unwrap();
        let second = generate(KeyAlgorithm::Ed25519, "", "").unwrap();
        assert_ne!(first.fingerprint, second.fingerprint);
    }

    /// 生成出来的那把钥匙，再解一遍问它自己是谁 —— 指纹要对得上，
    /// 算法要认得出来。保险箱存一份粘进来的私钥时走的就是 [`inspect`]。
    #[test]
    fn inspect_reads_back_what_generate_wrote() {
        for algorithm in [KeyAlgorithm::Ed25519, KeyAlgorithm::EcdsaP384] {
            let generated = generate(algorithm, "who@am-i", "").unwrap();
            let facts = inspect(&generated.private_key, "").unwrap();
            assert_eq!(facts.fingerprint, generated.fingerprint);
            assert_eq!(facts.public_key, generated.public_key);
            assert_eq!(facts.algorithm, Some(algorithm));
        }
    }

    /// 加密过的私钥要给对口令才问得出来 —— 和开会话时是同一条路。
    #[test]
    fn inspecting_an_encrypted_key_needs_the_passphrase() {
        let generated = generate(KeyAlgorithm::Ed25519, "", "hunter2").unwrap();
        assert!(inspect(&generated.private_key, "").is_err());
        let facts = inspect(&generated.private_key, "hunter2").unwrap();
        assert_eq!(facts.fingerprint, generated.fingerprint);
    }

    /// RSA 那一档也得真的生成得出来 —— 它是三档里唯一可能因为 crate feature
    /// 没开而在运行时报 `AlgorithmUnknown` 的（见 [`ssh_algorithm`]）。
    /// 慢，所以单独一条。
    #[test]
    fn rsa_is_available_too() {
        let generated = generate(KeyAlgorithm::Rsa4096, "rsa@test", "").unwrap();
        assert!(generated.public_key.starts_with("ssh-rsa "));
        decode_private_key(&generated.private_key, "").expect("RSA 私钥解不开");
    }