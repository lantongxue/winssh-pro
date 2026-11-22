//! Docker 容器管理后端：经**现有 SSH 会话**接 Docker Engine API。
//!
//! 不另开连接、不开本机监听口：每条 HTTP 连接是同一条 SSH 连接上的一条
//! `docker system dial-stdio` exec 通道（见 [`connector`]），字节流接进 bollard
//! 的自定义传输。权限即登录用户的 docker 权限，rootless 自动跟随，通道随会话生灭。
//!
//! | 文件 | 干什么 |
//! | --- | --- |
//! | [`connector`] | russh 通道 → hyper 流（bollard 自定义传输） |
//! | [`client`] | [`DockerClient`]：探针分类、容器/镜像/日志/stats/文件，bollard 模型折算 |
//! | [`types`] | 纯数据视图类型（不认识 bollard，也不认识界面） |
//! | [`listing`] | 容器文件列举：exec 脚本 + 解析 / archive tar 头解析 |
//! | [`path`] | 容器内路径规范化、拼接、shell quoting |
//!
//! 谁去开 SSH 通道、谁把读数给界面，不在这一层：那是 `winssh-service` 的
//! `DockerService` 与根包 UI 的事。这一层只认 `Handle` 与 bollard。

// 面向用户的文案走多语言查表，`t!` 展开成本 crate 的 `_rust_i18n_t!`；
// `../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR 指回仓库根那一份。
rust_i18n::i18n!("../../locales", fallback = "en");
const _: &str = include_str!("../../../locales/app.yml");

pub mod client;
pub mod connector;
pub mod listing;
pub mod path;
pub mod types;

pub use client::{ConnectError, DockerClient};
pub use connector::DialStdioConnector;
pub use types::{
    ContainerDetail, ContainerState, ContainerStats, ContainerSummary, EngineInfo, FileEntry,
    FileKind, ImageSummary, LogLine, Mount, PortMapping, PullProgress,
};
