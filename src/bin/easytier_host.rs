//! winssh-easytier-host：EasyTier 虚拟网卡那一档的提权 host。
//!
//! 本体在 [`winssh_easytier::host`]：建 wintun 网卡、配地址、加路由都要
//! 管理员权限，所以 GUI 没提权时把整张网交给这个进程（经 broker 拉起）。
//! 与 GUI 之间只有一条命名管道（`--pipe <名字>`），协议见
//! `winssh_easytier` 的 `host_protocol`。

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    winssh_easytier::host::main();
}
