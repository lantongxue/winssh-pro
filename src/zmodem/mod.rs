//! ZMODEM 会话层已下沉到 `winssh-zmodem`（见 [`winssh_zmodem`]）：`Session`（rz/sz
//! 状态机驱动器）、`Scanner`/签名嗅探、`Offer`/`Outcome`/`Progress` 与下载目录配置
//! （`config`）都在那里。这里只做再导出，保持 `crate::zmodem::Session`、
//! `crate::zmodem::config` 这些旧路径不变 —— 终端内核（`crate::terminal`）经它收发文件。

pub use winssh_zmodem::*;
