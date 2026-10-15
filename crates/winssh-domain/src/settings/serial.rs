//! 一台串口主机的线路参数：波特率、数据位、校验、停止位、流控，外加
//! 打开设备那一刻对 DTR / RTS 两条信号线的处置与本地回显开关。
//!
//! 这些全是 `serialport` 打开设备时要点开的旋钮，摆到主机表单的「串口」
//! 页里由用户自己拧：
//!
//! | 设置 | 线上是什么 | 什么时候动它 |
//! | --- | --- | --- |
//! | [`SerialBaud`] | termios / DCB 里的速率 | 对端 console 的速率不合就换（网络设备多为 9600，开发板多为 115200） |
//! | [`SerialDataBits`] / [`SerialParity`] / [`SerialStopBits`] | 一帧的位数与校验 | 出厂 8N1，老式设备会要 7E1 / 7O1 这类 |
//! | [`SerialFlowControl`] | RTS/CTS 或 XON/XOFF | 长线缆、高速率下防溢出；对端不配合时必须是「无」 |
//! | [`SerialLineState`]（DTR / RTS 各一格） | 两条信号线开端口那一刻的电平 | 有的设备要 RTS 拉高才回数据；有的开发板会被 DTR 跳变复位 |
//! | `local_echo` | 不在线上，只影响屏幕 | 对端不回显（裸 TTY、部分嵌入式 console）时替它把按键回声到屏幕 |
//!
//! 为什么 DTR / RTS 是**三态**而不是两个开关：出厂档必须是「不动」——
//! 跟随系统/驱动在打开端口时的默认行为。拉高会把某些开发板复位、拉低会
//! 让某些设备直接不再回数据 —— 那两种后果都只能由用户明确点开，不能是
//! 谁没配置就默认发生的。

use gpui_kit::SharedString;
use rust_i18n::t;
use serde::{Deserialize, Serialize};

/// 波特率。给常用档的预设表，而不是自由输入 —— 和全应用「下拉按下标
/// 折回枚举」的规矩一致；真需要冷门速率（250000 的 DMX 之类）时往枚举里
/// 加一档即可，存储格式不会因此变样。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerialBaud {
    B300,
    B1200,
    B2400,
    B4800,
    B9600,
    B19200,
    B38400,
    B57600,
    /// 出厂档：现代开发板与 USB 转串口最常见的速率。
    #[default]
    B115200,
    B230400,
    B460800,
    B921600,
}

impl SerialBaud {
    pub const ALL: [Self; 12] = [
        Self::B300,
        Self::B1200,
        Self::B2400,
        Self::B4800,
        Self::B9600,
        Self::B19200,
        Self::B38400,
        Self::B57600,
        Self::B115200,
        Self::B230400,
        Self::B460800,
        Self::B921600,
    ];

    /// 界面上的名字就是那个数，不译。也拿去当 `summary()` 的第一段。
    pub fn label(&self) -> SharedString {
        self.rate().to_string().into()
    }

    /// 送给 `serialport` 的那个数。
    pub fn rate(&self) -> u32 {
        match self {
            Self::B300 => 300,
            Self::B1200 => 1200,
            Self::B2400 => 2400,
            Self::B4800 => 4800,
            Self::B9600 => 9600,
            Self::B19200 => 19200,
            Self::B38400 => 38400,
            Self::B57600 => 57600,
            Self::B115200 => 115200,
            Self::B230400 => 230400,
            Self::B460800 => 460800,
            Self::B921600 => 921600,
        }
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// 一帧的数据位。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerialDataBits {
    Five,
    Six,
    Seven,
    /// 出厂档：8N1 的那个 8。
    #[default]
    Eight,
}

impl SerialDataBits {
    pub const ALL: [Self; 4] = [Self::Five, Self::Six, Self::Seven, Self::Eight];

    /// 界面上的名字就是那个数，不译。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Five => "5",
            Self::Six => "6",
            Self::Seven => "7",
            Self::Eight => "8",
        }
        .into()
    }

    pub fn data_bits(&self) -> serialport::DataBits {
        match self {
            Self::Five => serialport::DataBits::Five,
            Self::Six => serialport::DataBits::Six,
            Self::Seven => serialport::DataBits::Seven,
            Self::Eight => serialport::DataBits::Eight,
        }
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// 校验位。只给无 / 奇 / 偶三档：Mark / Space 校验在 macOS 的 termios 里
/// 没有对应的位（`CMSPAR` 只有 Linux 有），跨平台凑不齐的东西不摆出来。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerialParity {
    /// 出厂档：8N1 的那个 N。
    #[default]
    None,
    Odd,
    Even,
}

impl SerialParity {
    pub const ALL: [Self; 3] = [Self::None, Self::Odd, Self::Even];

    pub fn label(&self) -> SharedString {
        match self {
            Self::None => t!("serial.parity_none"),
            Self::Odd => t!("serial.parity_odd"),
            Self::Even => t!("serial.parity_even"),
        }
        .into()
    }

    /// `summary()` 里 `8N1` 那个字母。8N1 是串口的通用记号，不随界面语言走。
    pub fn abbr(&self) -> &'static str {
        match self {
            Self::None => "N",
            Self::Odd => "O",
            Self::Even => "E",
        }
    }

    pub fn parity(&self) -> serialport::Parity {
        match self {
            Self::None => serialport::Parity::None,
            Self::Odd => serialport::Parity::Odd,
            Self::Even => serialport::Parity::Even,
        }
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// 停止位。1.5 位在 POSIX 上要专门的标志、Windows 上两档都不齐，不做。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerialStopBits {
    /// 出厂档：8N1 的那个 1。
    #[default]
    One,
    Two,
}

impl SerialStopBits {
    pub const ALL: [Self; 2] = [Self::One, Self::Two];

    /// 界面上的名字就是那个数，不译。
    pub fn label(&self) -> SharedString {
        match self {
            Self::One => "1",
            Self::Two => "2",
        }
        .into()
    }

    pub fn stop_bits(&self) -> serialport::StopBits {
        match self {
            Self::One => serialport::StopBits::One,
            Self::Two => serialport::StopBits::Two,
        }
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// 流控。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerialFlowControl {
    /// 出厂档：对端不配合流控时开着它，字节会被双方各自吞掉一截。
    #[default]
    None,
    /// XON / XOFF（软件流控，0x11 / 0x13 两个字节）。
    Software,
    /// RTS / CTS（硬件流控，两条信号线）。
    Hardware,
}

impl SerialFlowControl {
    pub const ALL: [Self; 3] = [Self::None, Self::Software, Self::Hardware];

    pub fn label(&self) -> SharedString {
        match self {
            Self::None => t!("serial.flow_none"),
            Self::Software => t!("serial.flow_software"),
            Self::Hardware => t!("serial.flow_hardware"),
        }
        .into()
    }

    pub fn flow_control(&self) -> serialport::FlowControl {
        match self {
            Self::None => serialport::FlowControl::None,
            Self::Software => serialport::FlowControl::Software,
            Self::Hardware => serialport::FlowControl::Hardware,
        }
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// 开端口那一刻对一条信号线（DTR / RTS）的处置。
///
/// 「不动」是出厂档：`Default` 那一下**不调用**任何设置接口，行为就是
/// 系统/驱动打开串口时的默认动作（Linux 会拉起 DTR，Windows 不动）。
/// 别把「不动」当成「拉低」—— 那会把信号线主动按下去，是另一回事。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerialLineState {
    /// 不动，跟随系统默认。
    #[default]
    Default,
    /// 拉高（assert）。
    Assert,
    /// 拉低（clear）。
    Clear,
}

impl SerialLineState {
    pub const ALL: [Self; 3] = [Self::Default, Self::Assert, Self::Clear];

    pub fn label(&self) -> SharedString {
        match self {
            Self::Default => t!("serial.line_default"),
            Self::Assert => t!("serial.line_assert"),
            Self::Clear => t!("serial.line_clear"),
        }
        .into()
    }

    /// 要显式设置时的电平；`Default` 档没有电平可言。
    pub fn level(&self) -> Option<bool> {
        match self {
            Self::Default => None,
            Self::Assert => Some(true),
            Self::Clear => Some(false),
        }
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// 一台主机的串口参数。进 [`crate::host::HostDraft`]：和设备路径一样是
/// 「怎么连」的一部分，在主机表单里填。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialSettings {
    pub baud: SerialBaud,
    pub data_bits: SerialDataBits,
    pub parity: SerialParity,
    pub stop_bits: SerialStopBits,
    pub flow_control: SerialFlowControl,
    /// 开端口时 DTR 的处置，见 [`SerialLineState`]。
    pub dtr: SerialLineState,
    /// 开端口时 RTS 的处置，见 [`SerialLineState`]。
    pub rts: SerialLineState,
    /// 本地回显：用户键进去的字节要不要同时写一份到屏幕上。
    /// 和 Telnet 那套自适应嗅探不同 —— 串口的对端是块硬件，它回显不回显
    /// 用户自己清楚，给一个明确的开关就行（见 [`crate::serial`] 的模块文档）。
    pub local_echo: bool,
}

/// 出厂档：115200 8N1、无流控、DTR / RTS 都不动、不回显。
///
/// 手写而不是 derive：这一份是「这一列出现之前的老串口主机们」等效的配置，
/// 值要说得清、改得动（比如以后把默认波特率改成 9600），不能只看枚举里
/// 谁排第一。
impl Default for SerialSettings {
    fn default() -> Self {
        Self {
            baud: SerialBaud::B115200,
            data_bits: SerialDataBits::Eight,
            parity: SerialParity::None,
            stop_bits: SerialStopBits::One,
            flow_control: SerialFlowControl::None,
            dtr: SerialLineState::Default,
            rts: SerialLineState::Default,
            local_echo: false,
        }
    }
}

impl SerialSettings {
    /// `115200 8N1` 这样的摘要（流控非「无」时再拼一段）。
    /// 连接横幅、失败提示用它 —— 用户一眼就该看见「这一条是按什么参数开的」。
    pub fn summary(&self) -> String {
        let mut text = format!(
            "{} {}{}{}",
            self.baud.label(),
            self.data_bits.label(),
            self.parity.abbr(),
            self.stop_bits.label(),
        );
        match self.flow_control {
            SerialFlowControl::None => {}
            SerialFlowControl::Software => text.push_str(" · XON/XOFF"),
            SerialFlowControl::Hardware => text.push_str(" · RTS/CTS"),
        }
        text
    }
}

/// 把设置写成 `hosts.serial_settings` 那一列。
///
/// 出厂档写空串，和 [`crate::vnc::encode`]、[`crate::rdp::encode`] 同一个
/// 理由：绝大多数主机停在默认值上，一列空串比一段 JSON 好读，老库补出来
/// 的那一列也正好是它。
pub fn encode(settings: &SerialSettings) -> String {
    if *settings == SerialSettings::default() {
        return String::new();
    }
    serde_json::to_string(settings).unwrap_or_default()
}

/// 从 `hosts.serial_settings` 那一列读回设置。读不动就当出厂档 —— 这一列
/// 跟着主机一起读出来，为一段坏掉的 JSON 让整个列表打不开是不成比例的。
pub fn decode(raw: &str) -> SerialSettings {
    if raw.trim().is_empty() {
        return SerialSettings::default();
    }
    match serde_json::from_str(raw) {
        Ok(settings) => settings,
        Err(error) => {
            log::warn!("hosts.serial_settings 解析失败，这台串口主机按出厂档处理：{error}");
            SerialSettings::default()
        }
    }
}

#[cfg(test)]
#[path = "../tests/settings/serial.rs"]
mod tests;
