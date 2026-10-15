//! FTP / FTPS：照着一台 [`Host`](winssh_domain::host::Host) 把远端当文件面板用。
//! 协议由 suppaftp 收口，TLS 走 rustls（aws-lc-rs）。
//!
//! | 文件 | 干什么 |
//! | --- | --- |
//! | [`settings`] | 主机上那份 FTP 配置：明文 / 显式 FTPS / 隐式 FTPS |
//! | [`client`] | 拨号（直连或经代理）、TLS、登录、能力探测，以及连接池 |
//! | [`tls`] | 证书信任：系统证书库认的放行，不认的弹框确认、按指纹记进信任库 |
//! | [`listing`] | MLSD / LIST 两种目录列表折成面板要的条目，纯函数 |
//!
//! 文件面板要的那组方法（列目录、传输…）在 [`crate::service::FtpService`]，
//! 它站在这一层上面。
//!
//! 配置存在 [`FtpSettings`] 里，编码后落在 `hosts.ftp_settings` 那一列；
//! 地址、端口、账号、密码复用 `hosts` 表已有的那几列。

// 面向用户的错误文案走多语言查表，`t!` 展开成本 crate 的 `_rust_i18n_t!`；
// `../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR 指回仓库根。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

pub mod client;
pub mod listing;
pub mod tls;
mod settings;

pub use settings::{FtpSecurity, FtpSettings, decode, encode, port_after_switch};
