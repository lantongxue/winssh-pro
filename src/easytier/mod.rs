//! EasyTier 组网：照着一台 [`Host`](crate::host::Host) 把本机接进一张 EasyTier
//! 虚拟网，把这张网此刻的样子（本机节点、网里的其它节点、事件）递给界面。
//!
//! EasyTier 是一个去中心化的 P2P 组网工具（<https://github.com/EasyTier/EasyTier>），
//! 同样用 Rust 写成。这里不去起一个 `easytier-core` 子进程，而是把它的库直接
//! 嵌进来 —— 一个"主机"就是一个进程内的 EasyTier 实例，一个标签页就是它的
//! 状态面板。和 RDP 一样按职责分：
//!
//! - **配置**（[`settings`]）：节点地址、虚拟地址、接入方式、监听、选路、加密，
//!   摆在主机表单的「组网」页里。网络名称与密钥分别复用 `hosts.hostname` /
//!   `hosts.password` 两列（见那个模块的文档）。
//! - **翻译**（[`config`]）：[`settings`] 加上网络身份 → EasyTier 的 `TomlConfig`。
//! - **会话**（[`session`]）：一条会话一扇线程 + 一个 runtime，实例的起停、
//!   事件订阅与定时采样都在那里；界面通过 [`EasyTierInput`] /
//!   [`EasyTierOutput`] 两个通道跟它说话。
//! - **读数**（[`snapshot`]）：把 EasyTier 的 protobuf 快照换成我们自己的类型，
//!   界面不认识上游的任何一个类型。
//!
//! 界面上那扇窗是 [`crate::ui::panels::EasyTierPanel`]，这里只到"会话"为止。
//!
//! ## 两种接入方式
//!
//! 出厂是**用户态**：不建虚拟网卡（EasyTier 的 `no_tun`），在 `127.0.0.1` 上开一个
//! SOCKS5 入口，经它访问网里的虚拟 IP —— 不要管理员权限，本应用里的 SSH / RDP /
//! VNC 主机把代理指到这个入口就能直接连网里的机器。代价是单向的：用户态节点
//! 访问得到别人，别人却访问不到它的虚拟 IP（实测过：两个用户态节点之间经
//! SOCKS5 连对方的虚拟 IP 是连不通的，连自己的才通），所以要连的那一头得是
//! 以虚拟网卡接入的机器（服务器上装 EasyTier 的常规做法）。**虚拟网卡**那一档建一块
//! TUN 设备，整台机器都看得见这张网，代价是要以管理员（root）身份运行，
//! Windows 上还要 `wintun.dll` 在程序旁边；权限不够时启动失败，原因原样摆在
//! 标签页上。
//!
//! ## 这一版编进来的
//!
//! 隧道和 `easytier-core` 一样全：tcp / udp / ws / wss / quic / wg，外加
//! `txt://` / `srv://` / `http(s)://` 节点发现；节点地址认不认由 EasyTier 自己的
//! `TunnelScheme` 说了算（[`settings`]），监听端口照 `-l <端口>` 的规矩展开成
//! 每种协议一个。每一种都在回环上真连过一遍（[`session`] 的测试）。唯一没开的
//! 是 faketcp（要原始套接字，Windows 上还要 Npcap，理由见 `Cargo.toml`）。
//! 另有 UDP / TCP 打洞、DHCP 分配地址、SOCKS5 入口与虚拟网卡。
//!
//! 子网代理、端口转发、魔法 DNS、出口节点、KCP / QUIC 代理这几样 EasyTier 有、
//! 这里暂时没接 —— 要的时候往 [`settings`] 里加旋钮、在 [`config`] 里接一行
//! setter，再在 `Cargo.toml` 里点亮对应的特性。

mod config;
mod session;
mod settings;
mod snapshot;

pub use session::{EasyTierInput, EasyTierOutput, EasyTierSession};
pub use settings::{
    DEFAULT_LISTEN_PORT, DEFAULT_SOCKS5_PORT, EasyTierAccess, EasyTierSettings, decode, encode,
    split_peers,
};
pub use snapshot::{LogLine, LogTone, NetworkLog, NetworkSnapshot, PeerNode, PeerPath};
