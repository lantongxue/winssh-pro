//! WebDAV：照着一台 [`Host`](crate::host::Host) 把远端当文件面板用，
//! 以及备份同步的 WebDAV 那一档。协议由 `reqwest_dav` 收口（见 [`client`]）。
//!
//! 与 FTP / S3 是同一类"只开文件面板"的会话，但 WebDAV 恰好与两者都不同：
//!
//! - **有真目录**（像 FTP / SFTP）。目录是一等的资源，`MKCOL` 建、`DELETE` 删、
//!   `MOVE` 改，不像 S3 那样靠 key 前缀模拟。
//! - **没有连接池**。WebDAV 是跑在 HTTP 上的无状态请求-响应（`PROPFIND` /
//!   `GET` / `PUT` / `MOVE` …），一条连接同不同时只能跑一件事的问题根本不存在，
//!   `reqwest` 自己的连接池按主机复用 TCP 就够了 —— 不用像 FTP 那样
//!   自己养浏览道与传输道。
//! - **没有协议级的家目录**。服务器只认 URL 路径，`home()` 返回面板配的起点。
//! - **没有 chmod、没有属主**。WebDAV 的 `PROPFIND` 不回权限位，能拿到的是
//!   大小、修改时间与类型（是不是 collection）。
//!
//! 认证支持 Basic 与 Digest（见 [`AuthMode`]），够普通的 WebDAV
//! 服务器用（坚果云、Nextcloud、群晖、Apache mod_dav…）。TLS 走 rustls ——
//! `reqwest_dav` 的默认特性会带进 native-tls，所以 Cargo.toml 里把它关掉、
//! 只留 `rustls-tls`。
//!
//! | 文件 | 干什么 |
//! | --- | --- |
//! | [`settings`] | 主机上那份 WebDAV 配置：认证方式、根目录、是否跳过证书校验 |
//! | [`client`] | 建一只 `reqwest_dav::Client`（认证明细、超时、URL 拼装），以及路径折算 |
//! | [`store`] | 备份落点：列 / 写 / 读 / 删四个动作，与 [`client`] 只隔一层折算 |
//!
//! 文件面板要的那组方法（列目录、传输…）在 [`crate::service::WebdavService`]，
//! 它站在这一层上面。
//!
//! 配置存在 [`WebdavSettings`] 里，编码后落在 `hosts.webdav_settings` 那一列；
//! 端点 URL 落在 `hosts.hostname`，用户名 `hosts.user`，密码 `hosts.password`。

pub mod client;
mod settings;
pub mod store;

/// 测试用的回环 WebDAV 服务器（形状照着线上那台出过事的 openlist），
/// 连接层、备份落点与服务层的测试都拿它对着"真发出去的请求"验路径。
#[cfg(test)]
#[path = "../tests/webdav/fake_server.rs"]
pub mod fake_server;

pub use settings::{AuthMode, WebdavSettings, decode, encode};
pub use store::WebdavStore;
