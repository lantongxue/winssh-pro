//! 串口连接服务：UI 层打开一条串口会话、枚举本机串口设备的唯一入口。
//!
//! 与 [`crate::TelnetService`] 同一个角色 —— 界面按 [`crate::host::Host`]
//! 说要打开哪个设备，这一层把它翻译成一次实际的 `SerialConnection` 建立，
//! 不让 `serialport` 这类库渗进 `src/ui/`。
//!
//! 两个方法的底层都是**同步阻塞**的（打开设备要过一遍 termios / DCB 配置，
//! 枚举系统串口要翻设备表），都必须派到 `spawn_blocking` 上跑，否则 gpui
//! 线程会被卡住。用 [`crate::db::run`] + `tokio::task::spawn_blocking`
//! 桥接：`db::run` 让代码进应用级 tokio runtime、`spawn_blocking` 再把它
//! 踢到 blocking 池，两条链路都不占 worker。

use crate::{host::Host, serial::SerialConnection};

/// 串口连接的业务入口。无状态，廉价克隆。
#[derive(Clone, Default, Debug)]
pub struct SerialService;

impl SerialService {
    pub fn new() -> Self {
        Self
    }

    /// 按 [`Host`] 打开串口并起桥（设备路径在 `host.hostname`，参数在
    /// `host.serial`，见 [`crate::host::Host`]）。
    pub async fn connect(&self, host: &Host) -> anyhow::Result<SerialConnection> {
        let host = host.clone();
        crate::db::run(async move {
            tokio::task::spawn_blocking(move || SerialConnection::open_blocking(&host)).await?
        })
        .await
    }

    /// 本机当前可用的串口设备名（`/dev/cu.usbserial-0001`、`COM3`……），
    /// 给主机表单那个"检测到的串口"下拉用。
    ///
    /// macOS 上 `available_ports` 会同时列出 `/dev/cu.*` 与 `/dev/tty.*`
    ///（同一台设备的两条名字），出站连接该用 cu 那一份（tty 那份会被
    /// carrier detect 挂住），所以把 cu 稳定排到前面，组内仍按字母序。
    ///
    /// 枚举失败只记一条日志、返回空表 —— 它只是"帮用户少打几个字"，
    /// 为它打断表单填写不成比例；真正的失败在 [`Self::connect`] 里说。
    pub async fn list_ports(&self) -> Vec<String> {
        crate::db::run(async move {
            tokio::task::spawn_blocking(|| match serialport::available_ports() {
                Ok(ports) => {
                    let mut names: Vec<String> =
                        ports.into_iter().map(|port| port.port_name).collect();
                    names.sort();
                    // 稳定排序：`/dev/tty.*` 全部沉底，其余保持字母序。
                    names.sort_by_key(|name| name.contains("/dev/tty."));
                    names
                }
                Err(error) => {
                    log::warn!("枚举串口设备失败：{error}");
                    Vec::new()
                }
            })
            .await
            .unwrap_or_default()
        })
        .await
    }
}

#[cfg(test)]
#[path = "tests/service/serial_service.rs"]
mod tests;
