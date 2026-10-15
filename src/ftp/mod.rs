//! FTP/FTPS 后端已下沉到 `winssh-ftp`（见 [`winssh_ftp`]）：控制/数据连接、TLS
//! 会话复用、列表解析与证书校验都在那里，`FtpSettings` 经领域层再导出。这里只做
//! 再导出，保持 `crate::ftp::CertTrust`、`crate::ftp::FtpSettings` 这些旧路径不变 ——
//! 业务入口是 [`crate::service::FtpService`]。

pub use winssh_ftp::*;

// 连接层的集成测试跨 ftp + service + proxy + db（要调 `service::FtpService`），
// 留在根包最合适：这里能同时拿到这几个 crate，而 `winssh-ftp` 在服务层之下、
// 不该反向依赖它。`super::*` 经上面那行再导出拿到 `CertTrust` 等公开项。
#[cfg(test)]
#[path = "../tests/ftp/client.rs"]
mod client_tests;
