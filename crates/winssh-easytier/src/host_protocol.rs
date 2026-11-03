//! GUI 与提权 host（`winssh-easytier-host`）之间那条管道上说的话。
//!
//! 虚拟网卡那一档的实例必须跑在管理员权限下（建 wintun 网卡、配地址、
//! 加路由都不是普通用户干得了的），所以整张网被挪进一个经 broker 拉起的
//! host 进程，GUI 只在这头摆界面。两边用一行一条 JSON（NDJSON）说话：
//!
//! * GUI → host：先一条 [`Request::Start`] 把整份起网规格递过去，之后只有
//!   [`Request::Stop`]；
//! * host → GUI：会话本来就要往上送的那几种 [`Event`]。
//!
//! 快照与日志行直接复用 [`super::snapshot`] 的类型 —— 它们的 `SharedString`
//! 由那边的 serde 桥接模块转成普通字符串。协议只在同一版程序之间用（host
//! 是 GUI 旁边的同版本 sidecar），不做版本握手。

// 这套协议只在 Windows 的提权那一档真的用得上；别的平台照样编、照样跑
// serde 测试，未用条目不算问题。
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use super::settings::EasyTierSettings;
use super::snapshot::{LogLine, NetworkSnapshot};

/// 管道名的前缀。完整名字是 `winssh-easytier-<32 位小写十六进制>`。
pub const PIPE_PREFIX: &str = "winssh-easytier";

/// GUI → host 的指令。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// 照这份规格起网。一条连接只接受一次。
    Start(Box<Spec>),
    /// 优雅退出这张网（停实例、放掉网卡），然后 host 自己退出。
    Stop,
}

/// 起一张网要的全部东西。GUI 从 [`winssh_domain::host::Host`] 里拆出来，
/// host 用 [`super::config::build_parts`] 建出与进程内那一档一模一样的配置。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    /// 界面语言（`zh-CN` / `en`）。host 里的日志与错误文案按它翻译。
    pub locale: String,
    /// 组网主机的 id，拼实例名用。
    pub instance_id: String,
    /// 网络名称与密钥（分别是 `hosts.hostname` / `hosts.password` 两列）。
    pub network_name: String,
    pub secret: String,
    pub settings: EasyTierSettings,
}

/// host → GUI 的事件。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// 实例起来了：监听、虚拟网卡都已就位。
    Started,
    /// 一次采样。
    Snapshot(NetworkSnapshot),
    /// 事件日志的一行。
    Log(LogLine),
    /// 会话结束。`None` 是 GUI 要它停的；`Some` 是出了事，里面的原因已经
    /// 按 [`Spec::locale`] 翻成给用户看的话了。
    Stopped { reason: Option<String> },
}

/// 新生成一个管道名。随机后缀让别的本地进程猜不到、也撞不上同时开的
/// 第二条组网。
pub fn new_pipe_name() -> String {
    format!("{PIPE_PREFIX}-{:032x}", rand::random::<u128>())
}

/// host 从命令行拿到的那段管道名必须长成 [`new_pipe_name`] 的样子：
/// 它会被拼进 `\\.\pipe\…`，不校验就等于把路径拼接待到提权进程里。
pub fn validate_pipe_name(name: &str) -> Result<(), String> {
    let Some(suffix) = name.strip_prefix(PIPE_PREFIX) else {
        return Err(format!("pipe name must start with {PIPE_PREFIX}: {name:?}"));
    };
    let Some(hex) = suffix.strip_prefix('-') else {
        return Err(format!("pipe name has no suffix: {name:?}"));
    };
    if hex.len() != 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("pipe name suffix must be 32 hex digits: {name:?}"));
    }
    Ok(())
}

/// 把一条指令编成管道上的一行（不含换行符）。
pub fn encode_request(request: &Request) -> String {
    serde_json::to_string(request).expect("序列化组网 host 指令失败")
}

/// 把一个事件编成管道上的一行（不含换行符）。
pub fn encode_event(event: &Event) -> String {
    serde_json::to_string(event).expect("序列化组网 host 事件失败")
}

/// 认一行指令。认不出来就是对面发的不是我们认识的东西。
pub fn decode_request(line: &str) -> Result<Request, serde_json::Error> {
    serde_json::from_str(line)
}

/// 认一行事件。
pub fn decode_event(line: &str) -> Result<Event, serde_json::Error> {
    serde_json::from_str(line)
}

#[cfg(test)]
#[path = "tests/easytier/host_protocol.rs"]
mod tests;
