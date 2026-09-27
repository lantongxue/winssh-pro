//! 串口连接服务：UI 层打开一条串口会话、枚举本机串口设备的唯一入口。
//!
//! 与 [`crate::service::TelnetService`] 同一个角色 —— 界面按 [`crate::host::Host`]
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
mod tests {
    use super::*;
    use crate::host::{AuthMethod, HostKind, MonitorIntervals};

    fn missing_host() -> Host {
        Host {
            serial: Default::default(),
            easytier: Default::default(),
            kind: HostKind::Serial,
            id: "test-serial-missing".into(),
            name: "serial-missing".into(),
            user: "".into(),
            hostname: "/dev/winssh-pro-no-such-serial-port".into(),
            port: 0,
            platform: "".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            shell_integration: false,
        }
    }

    /// 与 Telnet 服务同一件事：在**没有 tokio reactor 的线程上**调用
    /// `connect`，它不该 panic 而是把错误原样递出来 —— 也就是
    /// `crate::db::run` 的那层壳起作用了。真正的打开错误归
    /// [`crate::serial::client`] 管。
    #[test]
    fn service_connect_without_tokio_reactor_does_not_panic() {
        let service = SerialService::new();
        let result = futures::executor::block_on(service.connect(&missing_host()));
        assert!(result.is_err(), "打不开的设备应返回错误而非 panic");
    }

    /// 枚举在任何一台机器上都该有个确定的结果（空表也是结果），
    /// 不 panic —— 它喂的是表单里的一个下拉。
    #[test]
    fn listing_ports_never_panics() {
        let service = SerialService::new();
        let ports = futures::executor::block_on(service.list_ports());
        // 不做内容断言：CI 上可能一个串口都没有。
        assert!(ports.len() < 4096, "枚举结果量级异常：{}", ports.len());
    }
}
