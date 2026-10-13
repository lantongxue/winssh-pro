//! FTPS 的证书信任：先问系统证书库，系统不认再走 SSH 那一套"首次确认、之后比对"。
//!
//! 两层，和 WinSCP 的做法一样：
//!
//! 1. **系统证书库认的证书直接放行**（Windows 上是系统的证书链引擎，与 reqwest
//!    走的是同一个 `rustls-platform-verifier`）。CA 签发、域名对得上、没过期的
//!    证书没什么好问用户的 —— 而且这种证书 Let's Encrypt 每两三个月就换一张，
//!    要是按指纹钉死，每次续期都会被当成中间人拦下来。
//! 2. **系统不认的**（自签名、内网 CA、过期、域名对不上）按指纹认，与 SSH 的
//!    主机密钥同一套规矩，记录也落在同一张 `trusted_hosts` 表里：
//!    - 库里没有 → 记下证书详情、中止握手，调用方弹确认框（凭据还没送出去 ——
//!      显式 FTPS 的 `AUTH TLS` 在 `USER` 之前，隐式 FTPS 更是一上来就是 TLS）；
//!    - 库里有且指纹相同 → 放行；
//!    - 库里有但指纹不同 → 拒绝，报"证书变了"，**不给"继续"的按钮**。和 SSH 的
//!      `KeyChanged` 一样：真是服务器换了证书，就到「受信任主机」里删掉那一条再连。
//!
//! 按指纹放行时**握手签名照样验**（[`TrustVerifier::verify_tls13_signature`] 那两个）：
//! 指纹只说明"对端出示的是这张证书"，签名才说明"对端手里有这张证书的私钥"。
//! 少了这一步，中间人把抓到的证书原样转出来就能过关。

use std::sync::{Arc, Mutex, OnceLock};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use sha2::{Digest as _, Sha256};
use x509_cert::Certificate;
use x509_cert::der::Decode as _;

use crate::trusted_host::HostKeyInfo;

/// 系统证书库为什么不认这张证书。确认框把它说成一句人话。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertProblem {
    /// 颁发者就是自己（`subject == issuer`）。vsftpd、FileZilla Server 默认就是这种。
    SelfSigned,
    /// 颁发者不在系统证书库里（内网 CA 多半是这种）。
    UnknownIssuer,
    Expired,
    NotYetValid,
    /// 证书上的域名 / IP 与连接用的地址对不上。
    NameMismatch,
    Revoked,
    /// 别的原因，或者系统证书库压根不可用。
    Other,
}

/// 握手时对端出示的那张证书，确认框与"证书变了"那句报错都用它。
#[derive(Clone, Debug)]
pub struct CertInfo {
    pub hostname: String,
    pub port: u16,
    /// `x509-rsa` / `x509-ecdsa` / `x509-ed25519`，见 [`key_type_of_oid`]。
    /// 和 SSH 的 `ssh-ed25519` 落在同一列，「受信任主机」那扇窗靠它分得清。
    pub key_type: String,
    /// `SHA256:AB:CD:…`，见 [`fingerprint`]。
    pub fingerprint: String,
    /// RFC 4514 的写法：`CN=ftp.example.com,O=Example`。解析不动的证书是空串。
    pub subject: String,
    pub issuer: String,
    /// 有效期两端，`YYYY-MM-DD`（UTC）。
    pub not_before: String,
    pub not_after: String,
    pub problem: CertProblem,
}

impl CertInfo {
    /// 折成信任库那一行要的形状。
    pub fn key_info(&self) -> HostKeyInfo {
        HostKeyInfo {
            hostname: self.hostname.clone(),
            port: self.port,
            key_type: self.key_type.clone(),
            fingerprint: self.fingerprint.clone(),
            old_fingerprint: None,
        }
    }
}

/// 一次 FTPS 会话的信任状态，由调用方备好、交给 [`crate::service::FtpService::connect`]。
///
/// 一次会话里所有连接（控制通道、连接池里后开的那几条、每一条数据通道）的
/// 握手都过同一个校验器、看同一份 `pinned`。
#[derive(Clone, Debug, Default)]
pub struct CertTrust {
    /// 认哪张证书：信任库里存的那个指纹，或者用户在确认框里点了"仅本次"的那个。
    /// `None` 是首次 —— 系统不认的证书一律记下来、中止握手。
    ///
    /// "仅本次"也钉住那一个指纹，不像 SSH 的 `TrustOnce` 那样什么都认：
    /// FTPS 一次会话里要握手很多次（每条数据通道一次），用户看过的只是第一张。
    pub pinned: Option<String>,
    /// 被拒的那张证书。首次时调用方从这里取出详情弹确认框；钉了指纹却对不上时
    /// 服务层从这里取出新指纹，报"证书变了"。
    pub rejected: Arc<Mutex<Option<CertInfo>>>,
}

impl CertTrust {
    pub fn pinned(fingerprint: String) -> Self {
        Self {
            pinned: Some(fingerprint),
            rejected: Arc::default(),
        }
    }

    /// 看一眼被拒的那张证书（不取走）。
    pub fn rejected(&self) -> Option<CertInfo> {
        self.rejected.lock().ok().and_then(|slot| slot.clone())
    }
}

/// 一张证书该怎么处置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// 系统证书库认它。
    Trusted,
    /// 系统不认，但指纹与钉住的那个相同。
    Pinned,
    /// 系统不认，也没钉过 —— 首次，要问用户。
    Unknown,
    /// 系统不认，钉过的指纹与这张对不上 —— 可能是中间人。
    Changed,
}

/// 处置规则本身，纯函数。见模块文档的那三条。
pub fn verdict(system_trusts: bool, pinned: Option<&str>, presented: &str) -> Verdict {
    if system_trusts {
        return Verdict::Trusted;
    }
    match pinned {
        Some(expected) if expected == presented => Verdict::Pinned,
        Some(_) => Verdict::Changed,
        None => Verdict::Unknown,
    }
}

/// 证书指纹：整张证书（DER）的 SHA-256，大写十六进制、冒号分隔，前缀 `SHA256:`。
///
/// 选这个写法而不是 SSH 那种 base64，是因为它和
/// `openssl x509 -noout -fingerprint -sha256`、FileZilla、浏览器的证书详情
/// 显示的是同一串 —— 用户要拿它去服务器上核对。
pub fn fingerprint(der: &[u8]) -> String {
    let digest = Sha256::digest(der);
    let hex: Vec<String> = digest.iter().map(|byte| format!("{byte:02X}")).collect();
    format!("SHA256:{}", hex.join(":"))
}

/// 公钥算法 OID → 信任库里那一列 `key_type`。
pub fn key_type_of_oid(oid: &str) -> &'static str {
    match oid {
        "1.2.840.113549.1.1.1" => "x509-rsa",
        "1.2.840.113549.1.1.10" => "x509-rsa-pss",
        "1.2.840.10045.2.1" => "x509-ecdsa",
        "1.3.101.112" => "x509-ed25519",
        "1.3.101.113" => "x509-ed448",
        _ => "x509",
    }
}

/// 系统证书库的报错 → 确认框上那一句。自签名在 Windows 上报的是"颁发者未知"，
/// 这里靠 `subject == issuer` 把它单独认出来 —— 那是最常见、也最该说清楚的一种。
pub fn problem_of(error: Option<&TlsError>, self_signed: bool) -> CertProblem {
    let certificate = match error {
        Some(TlsError::InvalidCertificate(certificate)) => certificate,
        _ if self_signed => return CertProblem::SelfSigned,
        _ => return CertProblem::Other,
    };
    match certificate {
        CertificateError::Expired | CertificateError::ExpiredContext { .. } => CertProblem::Expired,
        CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
            CertProblem::NotYetValid
        }
        CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
            CertProblem::NameMismatch
        }
        CertificateError::Revoked => CertProblem::Revoked,
        _ if self_signed => CertProblem::SelfSigned,
        CertificateError::UnknownIssuer => CertProblem::UnknownIssuer,
        _ => CertProblem::Other,
    }
}

/// 从 DER 里扒出确认框要显示的那几样。解析不动（极少见）就留空，指纹照样有。
fn describe(der: &[u8], hostname: &str, port: u16, error: Option<&TlsError>) -> CertInfo {
    let parsed = Certificate::from_der(der).ok();
    let (subject, issuer, not_before, not_after, key_type, self_signed) = match &parsed {
        Some(cert) => {
            let tbs = &cert.tbs_certificate;
            (
                tbs.subject.to_string(),
                tbs.issuer.to_string(),
                day_of(tbs.validity.not_before.to_unix_duration().as_secs()),
                day_of(tbs.validity.not_after.to_unix_duration().as_secs()),
                key_type_of_oid(&tbs.subject_public_key_info.algorithm.oid.to_string()),
                tbs.subject == tbs.issuer,
            )
        }
        None => (String::new(), String::new(), String::new(), String::new(), "x509", false),
    };
    CertInfo {
        hostname: hostname.to_string(),
        port,
        key_type: key_type.to_string(),
        fingerprint: fingerprint(der),
        subject,
        issuer,
        not_before,
        not_after,
        problem: problem_of(error, self_signed),
    }
}

fn day_of(unix_secs: u64) -> String {
    i64::try_from(unix_secs)
        .ok()
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map(|time| time.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// 进程里只要一份 aws-lc-rs 后端：握手、签名校验、系统证书库都用它。
/// 显式指名，不走"进程默认后端"那条路（见 [`crate::tls`] 的模块文档）。
fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .clone()
}

/// 系统证书库的校验器。建不起来（极少见：某些精简 Linux 没有证书库）就是
/// `None` —— 那时所有证书都走指纹那一层，不至于一张都连不上。
fn system_verifier() -> Option<Arc<dyn ServerCertVerifier>> {
    static SYSTEM: OnceLock<Option<Arc<dyn ServerCertVerifier>>> = OnceLock::new();
    SYSTEM
        .get_or_init(|| match rustls_platform_verifier::Verifier::new(provider()) {
            Ok(verifier) => Some(Arc::new(verifier) as Arc<dyn ServerCertVerifier>),
            Err(error) => {
                log::warn!("系统证书库不可用，FTPS 证书一律按指纹认：{error}");
                None
            }
        })
        .clone()
}

/// 两层校验器：系统证书库 + 指纹。见模块文档。
#[derive(Debug)]
pub struct TrustVerifier {
    system: Option<Arc<dyn ServerCertVerifier>>,
    trust: CertTrust,
    hostname: String,
    port: u16,
    provider: Arc<CryptoProvider>,
}

impl TrustVerifier {
    pub fn new(trust: CertTrust, hostname: &str, port: u16) -> Self {
        Self {
            system: system_verifier(),
            trust,
            hostname: hostname.to_string(),
            port,
            provider: provider(),
        }
    }
}

impl ServerCertVerifier for TrustVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let system = self.system.as_ref().map(|system| {
            system.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        });
        let system_error = match &system {
            Some(Err(error)) => Some(error),
            _ => None,
        };
        let presented = fingerprint(end_entity);
        match verdict(
            matches!(system, Some(Ok(_))),
            self.trust.pinned.as_deref(),
            &presented,
        ) {
            Verdict::Trusted | Verdict::Pinned => Ok(ServerCertVerified::assertion()),
            verdict => {
                if verdict == Verdict::Changed {
                    log::error!(
                        "FTPS 主机 {}:{} 证书变更！已存 {}，对端出示 {presented}",
                        self.hostname,
                        self.port,
                        self.trust.pinned.as_deref().unwrap_or_default(),
                    );
                }
                let info = describe(end_entity, &self.hostname, self.port, system_error);
                if let Ok(mut slot) = self.trust.rejected.lock() {
                    *slot = Some(info);
                }
                Err(TlsError::InvalidCertificate(
                    CertificateError::ApplicationVerificationFailure,
                ))
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// 一次会话的 TLS 客户端配置。所有连接共用这一份的校验器；会话缓存则每条
/// 控制连接各要一份（见 [`crate::ftp::client`] 里 `Site::connector` 的说明）。
pub fn client_config(
    trust: CertTrust,
    hostname: &str,
    port: u16,
) -> Result<rustls::ClientConfig, TlsError> {
    let verifier = Arc::new(TrustVerifier::new(trust, hostname, port));
    Ok(rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth())
}

#[cfg(test)]
#[path = "../tests/ftp/tls.rs"]
mod tests;
