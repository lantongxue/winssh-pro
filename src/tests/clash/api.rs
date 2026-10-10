    use super::*;

    #[test]
    fn path_segments_are_encoded() {
        assert_eq!(encode("PROXY"), "PROXY");
        assert_eq!(encode("香港 01"), "%E9%A6%99%E6%B8%AF%2001");
        assert_eq!(encode("a/b"), "a%2Fb");
    }

    /// 没密钥就不发 `Authorization`，有密钥就发 —— 控制口只绑回环，
    /// 这一格是防本机别的程序顺手读走节点列表的。
    #[test]
    fn the_secret_is_only_sent_when_there_is_one() {
        let with = ApiClient::new(9, "s3cret");
        let request = with
            .request(reqwest::Method::GET, "/version")
            .build()
            .unwrap();
        assert_eq!(
            request.headers().get("authorization").unwrap(),
            "Bearer s3cret"
        );

        let without = ApiClient::new(9, "");
        let request = without
            .request(reqwest::Method::GET, "/version")
            .build()
            .unwrap();
        assert!(request.headers().get("authorization").is_none());
    }
