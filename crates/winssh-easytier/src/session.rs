//! 一条活着的组网会话：一个嵌在本进程里的 EasyTier 实例，外加盯着它的那条循环。
//!
//! **每条会话一扇自己的 OS 线程 + 一个自己的多线程 tokio runtime**，不进应用级
//! runtime（[`crate::db::runtime`]）。EasyTier 一个实例会在当前 runtime 上铺开
//! 几十个常驻任务（监听、打洞、路由同步、SOCKS5 入口、心跳……），这些任务的
//! 寿命该和这张网绑在一起：网断了，整个 runtime 一起拆掉，漏掉一两个没收尾的
//! 任务也被 `shutdown_timeout` 兜走，不会留在数据层那个 runtime 上悄悄跑下去。
//! 拆 runtime 本身会等 blocking 任务，放在界面线程上就是卡一下 —— 所以放在这条
//! 线程的末尾做。形状和 RDP 那条会话线程一样（见 [`crate::rdp`]），界面与
//! 循环之间只走 mpsc 通道，通道跨 executor 无碍。
//!
//! 循环里三件事并排跑：
//!
//! - **事件**：订阅实例的事件总线（节点加入/离开、连接建立/断开、监听、
//!   DHCP 分到地址……），翻成一句人话的 [`LogLine`] 送给界面；
//! - **采样**：每 [`SNAPSHOT_INTERVAL`] 取一次运行快照（和 `easytier-cli peer`
//!   读的是同一份），翻成 [`NetworkSnapshot`]、补上跨两次采样的速率送出去；
//! - **指令**：界面说停就停。通道断了（面板没了）也当停。
//!
//! 实例自己死了（虚拟网卡被拔、监听口被抢这类致命错）也在采样那一拍发现：
//! 状态落到 `Stopped`，带着它最后一句错误报 [`EasyTierOutput::Stopped`]。
//!
//! 同样的几件事也一路报给入口登记处（[`super::portal`]）：起来了、每一拍走得到
//! 哪些地址、要停了、怎么结束的 —— 经这张网去连的别的主机在那边等它。

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use easytier::common::global_ctx::{EventBusSubscriber, GlobalCtxEvent};
use easytier::instance::factory::{create_native_instance, subscribe_native_instance_event};
use easytier_core::config::toml::TomlConfig;
use easytier_core::instance::CoreInstanceState;
use easytier_core::management::network_instance_running_info;
use gpui_kit::SharedString;
use rust_i18n::t;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use super::config;
use super::portal::{self, Ticket};
use super::settings::EasyTierAccess;
use super::snapshot::{LogLine, LogTone, NetworkSnapshot};
use winssh_domain::host::Host;

/// 多久取一次运行快照。一秒一拍：时延、速率这几个数要"看得见在动"；
/// 取一次只是读几把锁里的表，不上线，不值得为省它放慢。
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);

/// 启动最多等多久。建虚拟网卡、绑监听口、起 SOCKS5 入口都在这一步里；
/// 超过它多半是卡在了系统那一侧（权限弹窗没人点、驱动装不上）。
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// 停下最多等多久。到点就不等了：runtime 一拆，剩下的任务一并收走。
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// 拆 runtime 时给还没收尾的任务留的时间。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// 这张网用的 worker 数。一台桌面客户端上的一张网，流量不过是几条 SSH /
/// 远程桌面，两条足够；给多了只是多几条空转的线程。
const WORKER_THREADS: usize = 2;

/// 界面要会话做的事。
#[derive(Debug)]
pub enum EasyTierInput {
    /// 退出这张网。
    Stop,
}

/// 会话送给界面的东西。
#[derive(Debug)]
pub enum EasyTierOutput {
    /// 实例起来了：监听、SOCKS5 入口 / 虚拟网卡都已就位，开始找节点。
    Started,
    /// 一次采样。
    Snapshot(Box<NetworkSnapshot>),
    /// 事件日志的一行。
    Log(LogLine),
    /// 会话结束。`None` 是界面要它停的；`Some` 是出了事，带着原因。
    Stopped(Option<SharedString>),
}

/// 一条组网会话的把手。
pub struct EasyTierSession {
    input_tx: mpsc::UnboundedSender<EasyTierInput>,
    events_rx: Option<mpsc::UnboundedReceiver<EasyTierOutput>>,
}

impl EasyTierSession {
    /// 照主机配置起一张网。配置本身不对（网络名称空着、节点地址认不得）
    /// 当场返回错误；实例起没起得来走事件 —— [`EasyTierOutput::Started`] 或
    /// [`EasyTierOutput::Stopped`]。
    pub fn start(host: &Host) -> Result<Self> {
        // wss 隧道与 https / txt 节点发现要建 TLS；测试里不经 `main`，这里再兜一次。
        winssh_tls::install_default_provider();
        // 先登记再校验：配置不对也要让等着这张网的拨号当场知道，别干等到超时。
        let ticket = portal::open(host.id.clone(), host.name.clone());
        let config = match config::build(host) {
            Ok(config) => config,
            Err(error) => {
                ticket.finish(Some(format!("{error:#}").into()));
                return Err(error);
            }
        };
        let socks5 = host
            .easytier
            .socks5_address()
            .and_then(|address| address.parse::<SocketAddr>().ok());
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();

        let _ = events_tx.send(EasyTierOutput::Log(LogLine::now(
            LogTone::Info,
            match host.easytier.socks5_address() {
                Some(address) => t!("easytier.log_starting_socks5", address => address),
                None => t!("easytier.log_starting_tun"),
            },
        )));

        let thread_name = format!("easytier-{}", host.id);
        let access = host.easytier.access;
        // 线程起不来的话闭包连同登记那一格一起掉在地上，登记处当它出了事。
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || session_thread(config, access, socks5, ticket, input_rx, events_tx))
            .map_err(|error| anyhow!("启动组网会话线程失败：{error}"))?;

        Ok(Self {
            input_tx,
            events_rx: Some(events_rx),
        })
    }

    /// 指令通道的一份克隆。
    pub fn inputs(&self) -> mpsc::UnboundedSender<EasyTierInput> {
        self.input_tx.clone()
    }

    /// 取走事件接收端。只能取一次 —— 事件泵只该有一个。
    pub fn take_events(&mut self) -> Option<mpsc::UnboundedReceiver<EasyTierOutput>> {
        self.events_rx.take()
    }
}

fn session_thread(
    config: TomlConfig,
    access: EasyTierAccess,
    socks5: Option<SocketAddr>,
    ticket: Ticket,
    input_rx: mpsc::UnboundedReceiver<EasyTierInput>,
    events_tx: mpsc::UnboundedSender<EasyTierOutput>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WORKER_THREADS)
        .thread_name("easytier-worker")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let reason = SharedString::from(t!("easytier.runtime_failed", error => error));
            ticket.finish(Some(reason.clone()));
            let _ = events_tx.send(EasyTierOutput::Stopped(Some(reason)));
            return;
        }
    };
    let outcome = runtime.block_on(run(config, access, socks5, &ticket, input_rx, &events_tx));
    // `Stopped` 等 runtime 拆完才报：监听口、SOCKS5 入口这时候才真的放掉，
    // 界面收到它就能立刻重新连接，不会撞上自己上一次还没松手的端口。
    runtime.shutdown_timeout(SHUTDOWN_GRACE);
    ticket.finish(outcome.clone());
    let _ = events_tx.send(EasyTierOutput::Stopped(outcome));
}

/// 这张网的一生。返回的是它怎么结束的：`None` 是要它停的，`Some` 是出了事。
/// 一路的状态同时报给入口登记处（`ticket`）。
async fn run(
    config: TomlConfig,
    access: EasyTierAccess,
    socks5: Option<SocketAddr>,
    ticket: &Ticket,
    mut input_rx: mpsc::UnboundedReceiver<EasyTierInput>,
    events_tx: &mpsc::UnboundedSender<EasyTierOutput>,
) -> Option<SharedString> {
    let send = |output: EasyTierOutput| {
        let _ = events_tx.send(output);
    };

    // 建实例要在 runtime 里：它当场就往当前 runtime 上铺任务。
    let instance = match create_native_instance(config) {
        Ok(instance) => instance,
        Err(error) => return Some(failure(&error, access)),
    };
    // 先订阅再启动：启动那一下的动静（开始监听、正在连接……）也要看得到。
    let mut events = subscribe_native_instance_event(&instance);

    let started = tokio::select! {
        started = tokio::time::timeout(START_TIMEOUT, instance.start()) => started,
        // 启动期间用户就点了停（或者面板关了）：不等它起完。
        _ = input_rx.recv() => {
            ticket.stopping();
            stop(&instance).await;
            return None;
        }
    };
    match started {
        Ok(Ok(())) => {
            ticket.running(socks5);
            send(EasyTierOutput::Started);
        }
        Ok(Err(error)) => {
            stop(&instance).await;
            return Some(failure(&error, access));
        }
        Err(_) => {
            stop(&instance).await;
            return Some(t!("easytier.start_timeout", secs => START_TIMEOUT.as_secs()).into());
        }
    }

    let mut ticker = tokio::time::interval(SNAPSHOT_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut previous: Option<((u64, u64), Instant)> = None;
    let outcome = loop {
        tokio::select! {
            command = input_rx.recv() => match command {
                Some(EasyTierInput::Stop) | None => break None,
            },
            Some(event) = next_event(&mut events) => {
                if let Some(line) = LogLine::from_event(&event) {
                    send(EasyTierOutput::Log(line));
                }
            }
            _ = ticker.tick() => {
                if events_tx.is_closed() {
                    break None;
                }
                if instance.state() == CoreInstanceState::Stopped {
                    let reason = instance
                        .latest_error()
                        .filter(|error| !error.trim().is_empty())
                        .unwrap_or_else(|| t!("easytier.stopped_unexpectedly").to_string());
                    break Some(SharedString::from(reason));
                }
                match network_instance_running_info(instance.as_ref()).await {
                    Ok(info) => {
                        let mut snapshot = NetworkSnapshot::from_running_info(&info);
                        ticket.reach(snapshot.reach());
                        let totals = snapshot.total_bytes();
                        let now = Instant::now();
                        snapshot.rate_kbps = previous.map(|(before, at)| rates(before, totals, now - at));
                        previous = Some((totals, now));
                        send(EasyTierOutput::Snapshot(Box::new(snapshot)));
                    }
                    Err(error) => log::warn!("取组网运行快照失败：{error:#}"),
                }
            }
        }
    };

    // 要它停的：先从登记处拿掉，收尾那几秒里别再有拨号往里走。
    if outcome.is_none() {
        ticket.stopping();
    }
    stop(&instance).await;
    outcome
}

/// 事件总线的下一条。落后太多（界面那边不会，但总线容量有限）就跳过丢掉的
/// 那一段接着读；总线关了就永远不再来 —— 交给 `select!` 的另外两支。
async fn next_event(events: &mut Option<EventBusSubscriber>) -> Option<GlobalCtxEvent> {
    loop {
        let Some(receiver) = events.as_mut() else {
            return std::future::pending().await;
        };
        match receiver.recv().await {
            Ok(event) => return Some(event),
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => *events = None,
        }
    }
}

/// 停实例，最多等 [`STOP_TIMEOUT`]。等不到就算了：runtime 一拆全收走。
async fn stop(instance: &std::sync::Arc<easytier::instance::factory::NativeCoreInstance>) {
    if tokio::time::timeout(STOP_TIMEOUT, instance.stop())
        .await
        .is_err()
    {
        log::warn!("组网实例 {STOP_TIMEOUT:?} 内没停下来，随 runtime 一起拆掉");
    }
}

/// 起不来的原因。虚拟网卡那一档最常见的是没权限，原文（`Operation not
/// permitted`、`拒绝访问`）不说该怎么办，补一句。
fn failure(error: &anyhow::Error, access: EasyTierAccess) -> SharedString {
    let detail = format!("{error:#}");
    match access {
        EasyTierAccess::Tun => t!("easytier.start_failed_tun", error => detail).into(),
        EasyTierAccess::Socks5 => t!("easytier.start_failed", error => detail).into(),
    }
}

/// 两次采样之间的平均收/发速率，KB/s。计数回落（有节点断开，它那几条连接的
/// 字节跟着没了）按 0 算，不报一个负数；窗口下限挡住除以零。
fn rates(before: (u64, u64), after: (u64, u64), elapsed: Duration) -> (f32, f32) {
    let secs = elapsed.as_secs_f64().max(0.001);
    let rate =
        |before: u64, after: u64| (after.saturating_sub(before) as f64 / 1024. / secs) as f32;
    (rate(before.0, after.0), rate(before.1, after.1))
}

#[cfg(test)]
#[path = "tests/easytier/session.rs"]
mod tests;
