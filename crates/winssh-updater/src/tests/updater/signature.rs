//! 签名与校验和。这里只测不依赖真实密钥的部分 —— 生成一把 minisign 私钥
//! 属于发版流程，不该进单测；真实签名的端到端验证放在发版后的人工验证里。

use super::*;

#[test]
fn sha256_matches_known_vector() {
    // "abc" 的 sha256，到处都有的标准测试向量。
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn checksum_mismatch_is_rejected() {
    let error = verify_download(b"abc", "00", "sig").unwrap_err();
    assert!(error.to_string().contains("校验和不匹配"), "{error:#}");
}

#[test]
fn empty_checksum_skips_the_checksum_but_still_needs_a_signature() {
    // 没有 sha256 时不能因此直接通过：签名那道关必须过。
    let error = verify_download(b"abc", "", "not a signature").unwrap_err();
    assert!(error.to_string().contains("签名"), "{error:#}");
}

#[test]
fn build_without_public_key_cannot_verify() {
    // 仓库里公钥还是空的（维护者生成密钥后填进去）。空公钥必须验不过，
    // 否则"没有签名"会被误当成"验签通过"。
    if PUBLIC_KEY.trim().is_empty() {
        assert!(verify(b"abc", "sig", false).is_err());
    }
}
