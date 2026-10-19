//! 签名验证：minisign（ed25519）。
//!
//! 自动更新的信任根是**编译进二进制的公钥**，不是 HTTPS。更新接口按它的
//! 返回挑包，但装下去之前客户端自己验一遍：签名对不上，版本号与文件名全都
//! 不可信，直接丢弃。
//!
//! 公钥由维护者用 `minisign -G` 生成一次：
//!
//! ```text
//! minisign -G -p winssh-pro.pub -s winssh-pro.key   # 本地生成
//! gh secret set MINISIGN_SECRET_KEY < winssh-pro.key
//! ```
//!
//! 然后把 `winssh-pro.pub` 里 base64 那一行贴到下面的 [`PUBLIC_KEY`]。
//! 私钥只存在于 Actions Secret 里，发版 job 给每个产物出 `<文件>.minisig`
//! （见 `.github/workflows/release.yml` 的 Sign release assets 一步）。
//!
//! **公钥为空时**（还没生成密钥）验签一律失败，调用方据此退化成"只提示、
//! 不自动安装" —— 这样先合并 CI、之后再补密钥也不会让客户端装下来路不明
//! 的东西。

use anyhow::{Context as _, Result, bail};
use minisign_verify::{PublicKey, Signature};
use sha2::{Digest as _, Sha256};

/// 更新包签名用的公钥（minisign 的 base64 公钥串，不含 `untrusted comment` 行）。
///
/// 还没有密钥时保持空串。
pub const PUBLIC_KEY: &str = "RWQ5oooCz1yYIMw/d71+dFPX2CLbY+c9QqCsxFTJ4aWV1FxfPYNZidXp";

/// 编译期内嵌的公钥，解析不了就是 `None`。
pub fn public_key() -> Option<PublicKey> {
    let raw = PUBLIC_KEY.trim();
    if raw.is_empty() {
        return None;
    }
    PublicKey::from_base64(raw).ok()
}

/// 验一遍签名。`signature_text` 是 `.minisig` 文件的全文。
pub fn verify(bytes: &[u8], signature_text: &str, prehashed: bool) -> Result<()> {
    let Some(key) = public_key() else {
        bail!("这个版本的客户端还没有更新包公钥，无法验证签名");
    };
    let signature = Signature::decode(signature_text).context("签名文件格式不对")?;
    key.verify(bytes, &signature, prehashed)
        .context("更新包签名验证失败")?;
    Ok(())
}

/// 小写十六进制的 sha256。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 校验下载下来的文件：先比接口给的 sha256，再验签名。
///
/// sha256 防的是传输损坏与服务端笔误；签名防的是"接口和下载源一起被人
/// 换掉"。两道都要有：只比 sha256 的话，换掉接口的人顺手换掉哈希就行。
pub fn verify_download(bytes: &[u8], expected_sha256: &str, signature_text: &str) -> Result<()> {
    if !expected_sha256.is_empty() {
        let actual = sha256_hex(bytes);
        if !actual.eq_ignore_ascii_case(expected_sha256.trim()) {
            bail!("更新包校验和不匹配（期望 {expected_sha256}，实际 {actual}）");
        }
    }
    verify(bytes, signature_text, false)
}

#[cfg(test)]
#[path = "tests/updater/signature.rs"]
mod tests;
