//! 生成一对新的 SSH 密钥。
//!
//! 原型里这一步是假的（往框里塞一段随机 base64），在这里它是真的：
//! `ssh-keygen -t ed25519 -C ...` 生成的是什么，这里生成的就是什么 ——
//! 同一套 OpenSSH 私钥格式、同一种公钥行、同一个 SHA256 指纹，
//! 生成完的公钥贴进对端的 `authorized_keys` 就能登录。
//!
//! 底下是 `ssh-key`（`russh::keys` 转出来的那一份，和
//! [`super::decode_private_key`] 解私钥用的是同一个库），所以"生成出来的
//! 东西自己解得开"是有保证的 —— 这一条有测试钉着。
//!
//! **这里的每一步都是 CPU 密集的**，RSA 4096 那一档尤其：找两个 2048 位的
//! 素数要好几秒，加密私钥还要再过一遍故意设计得慢的 bcrypt-pbkdf。所以这个
//! 函数是同步的，由 [`crate::service::SshService::generate_key`] 派到
//! `spawn_blocking` 上去跑，绝不能在 gpui 线程上直接调。

use anyhow::{Context as _, Result};
use russh::keys::{Algorithm, EcdsaCurve, HashAlg, PrivateKey, ssh_key::LineEnding};
use rust_i18n::t;

use crate::credential::KeyAlgorithm;

/// 刚生成出来的一对密钥。三样东西正好填满保险箱里私钥那一档的三格。
#[derive(Clone, Debug)]
pub struct GeneratedKey {
    /// 公钥那一行：`ssh-ed25519 AAAA... comment`，贴进 `authorized_keys` 的就是它。
    pub public_key: String,
    /// 私钥那一整份 PEM 文本。给了口令的话，这份是**加密过**的。
    pub private_key: String,
    /// `SHA256:...` 形式的公钥指纹，和 `ssh-keygen -lf` 打出来的那一串一致。
    pub fingerprint: String,
}

/// 生成一对密钥。
///
/// `comment` 是公钥行尾那段注释（`ssh-keygen -C`），留空就没有注释。
/// `passphrase` 留空就是"这把私钥不加密"，和 `ssh-keygen -N ""` 一样 ——
/// 不是拿空串当密码去加密一遍，那样解的时候会被当成"要密码的私钥"。
///
/// 换行统一用 LF：私钥 PEM 的行尾在各家实现里都是 LF，Windows 上跟着系统
/// 写成 CRLF 的话，那份文本贴到 Linux 上就解不开了。
pub fn generate(algorithm: KeyAlgorithm, comment: &str, passphrase: &str) -> Result<GeneratedKey> {
    let mut rng = rand::rng();
    let mut key = PrivateKey::random(&mut rng, ssh_algorithm(algorithm)).with_context(|| {
        t!("keygen.generate_failed", algorithm => algorithm.badge()).to_string()
    })?;

    let comment = comment.trim();
    if !comment.is_empty() {
        key.set_comment(comment);
    }

    // 指纹与公钥都在**加密之前**取：加密只改私钥那一份文本的封装，
    // 公钥与指纹是同一把钥匙的另一面，不受影响。放在这里取是因为
    // `encrypt` 之后的那个 `PrivateKey` 里公钥部分虽然还在，
    // 但"先取再加密"读起来才是这件事本来的顺序。
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    let public_key = key
        .public_key()
        .to_openssh()
        .with_context(|| t!("keygen.encode_public_failed").to_string())?;

    // 空口令不等于"用空串加密"：那样存下来的是一把加密过的私钥，
    // 而解它的时候我们传的是 `None`（见 `decode_private_key`），解不开。
    let key = match passphrase.is_empty() {
        true => key,
        false => key
            .encrypt(&mut rng, passphrase)
            .with_context(|| t!("keygen.encrypt_failed").to_string())?,
    };
    let private_key = key
        .to_openssh(LineEnding::LF)
        .with_context(|| t!("keygen.encode_private_failed").to_string())?
        .to_string();

    Ok(GeneratedKey {
        public_key,
        private_key,
        fingerprint,
    })
}

/// 一份**已有**私钥自己交代的那几件事。
///
/// 保险箱在存一份粘进来的私钥之前问一次：指纹是多少、它的公钥长什么样、
/// 是哪种算法。原型里这三样是编出来的（一串随机十六进制冒充 SHA256），
/// 在这里它们是从那把钥匙本身算出来的 —— 卡片上那行指纹要能和对端
/// `authorized_keys` 里的那一行对上，编出来的对不上。
#[derive(Clone, Debug)]
pub struct KeyFacts {
    pub fingerprint: String,
    pub public_key: String,
    /// 认出来是哪一档就给哪一档；DSA、P-256 这些界面上没有的档给 `None`，
    /// 由调用方保留用户自己选的那一档。
    pub algorithm: Option<KeyAlgorithm>,
}

/// 解开一份私钥，把它自己交代的那几件事取出来。
///
/// 和 [`super::decode_private_key`] 走同一条路，所以"这份私钥认不认得"
/// 与开会话时的答案一致；加密过的私钥要给对口令，否则解不开。
/// 同样是 CPU 密集的（bcrypt-pbkdf），调用方负责派到 blocking 线程上。
pub fn inspect(content: &str, passphrase: &str) -> Result<KeyFacts> {
    let key = super::decode_private_key(content, passphrase)?;
    Ok(KeyFacts {
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        public_key: key
            .public_key()
            .to_openssh()
            .with_context(|| t!("keygen.encode_public_failed").to_string())?,
        algorithm: match key.algorithm() {
            Algorithm::Ed25519 => Some(KeyAlgorithm::Ed25519),
            Algorithm::Rsa { .. } => Some(KeyAlgorithm::Rsa4096),
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP384,
            } => Some(KeyAlgorithm::EcdsaP384),
            _ => None,
        },
    })
}

/// 界面上那三档对应到 `ssh-key` 的算法。
///
/// RSA 那一档不带哈希（`hash: None`）：签名时用哪个哈希是**连接时**跟对端
/// 协商出来的（见 [`super::connection`] 里的 `best_supported_rsa_hash`），
/// 不是生成密钥时定死的。位长也不在这里选 —— `ssh-key` 的 `random` 固定生成
/// 4096 位，这正是界面上那一档写着的数。
fn ssh_algorithm(algorithm: KeyAlgorithm) -> Algorithm {
    match algorithm {
        KeyAlgorithm::Ed25519 => Algorithm::Ed25519,
        KeyAlgorithm::Rsa4096 => Algorithm::Rsa { hash: None },
        KeyAlgorithm::EcdsaP384 => Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP384,
        },
    }
}

#[cfg(test)]
#[path = "../tests/ssh/keygen.rs"]
mod tests;
