//! 凭据（保险箱）的数据模型已下沉到 `winssh-domain`（见
//! [`winssh_domain::credential`]）：`Credential`、`CredentialKind`、`KeyAlgorithm`、
//! `CredentialDraft`、`PasswordRecipe`、`PasswordStrength` 与 tags / 备份 JSON 的
//! 编解码都在那里。这里只做再导出，保持 `crate::credential::Credential` 这样的
//! 旧路径不变 —— 落库归 [`crate::service::CredentialService`]，真正生成密钥对归
//! [`crate::ssh::keygen`]。

pub use winssh_domain::credential::*;
