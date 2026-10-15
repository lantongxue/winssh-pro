//! 一条串口会话的闭环：`打开设备 → 设参数 → 起桥`。
//!
//! [`SerialConnection::open_blocking`] 是这条路径的入口。它**同步阻塞**
//! 地打开设备（`open()` 里含 termios / DCB 那一串配置，正常情况下是个
//! 毫秒级动作，但设备被占 / 权限不足 / 节点不存在都要在这里给出错误），
//! 所以必须跑在 `tokio::task::spawn_blocking` 上（见
//! [`crate::service::SerialService::connect`]），不能直接在 GPUI 线程调用。
//!
//! 与 SSH 那边不同，这里没有可以再开 shell 的 `session` 句柄：一条串口
//! 就是一条会话，没有复用这一说。所以 [`SerialConnection`] 与
//! [`crate::telnet::TelnetConnection`] 同形 —— 一份桥 + 一条输出接收端。

use anyhow::{Context as _, Result};
use rust_i18n::t;

use super::bridge::{READ_POLL, SerialBridge, SerialOutputReceiver, spawn_bridge};
use winssh_domain::host::Host;

/// 一条活着的串口会话：一份桥 + 一条 GPUI 侧的输出接收端。
pub struct SerialConnection {
    pub bridge: SerialBridge,
    pub output_rx: SerialOutputReceiver,
}

impl SerialConnection {
    /// 按 [`Host`] 打开串口、配好参数、把桥起起来。
    ///
    /// 只在 `spawn_blocking` 上调用；设备打开并交付两条线程之后，本函数
    /// 本身不再持有任何等待。
    pub fn open_blocking(host: &Host) -> Result<Self> {
        let settings = host.serial;
        // 串口主机的"地址"就是设备路径，存在 `hostname` 列里
        //（见 [`winssh_domain::host::Host::address`]）。
        let device = host.hostname.as_str();

        let mut builder = serialport::new(device, settings.baud.rate())
            .data_bits(settings.data_bits.data_bits())
            .parity(settings.parity.parity())
            .stop_bits(settings.stop_bits.stop_bits())
            .flow_control(settings.flow_control.flow_control())
            // 读要超时轮询：这是桥的关闭标志唯一的检查点。
            .timeout(READ_POLL);
        // DTR：构建器有"开的时候置成什么"这一档；「不动」就不传，
        // 走系统默认（Linux 打开时自动拉起，Windows 不动）。
        if let Some(level) = settings.dtr.level() {
            builder = builder.dtr_on_open(level);
        }

        let mut port = builder
            .open()
            .with_context(|| t!("serial.open_failed", device => device).to_string())?;

        // RTS：构建器没有"开的时候置成什么"这一档，开完再设。
        if let Some(level) = settings.rts.level() {
            port.write_request_to_send(level).with_context(|| {
                t!("serial.line_failed", line => "RTS", device => device).to_string()
            })?;
        }

        let (bridge, output_rx) = spawn_bridge(port, settings.local_echo)?;
        Ok(Self { bridge, output_rx })
    }
}

#[cfg(test)]
#[path = "tests/serial/client.rs"]
mod tests;
