//! 组网与别的主机之间的那座桥：本进程里此刻有哪几张网、各自从哪儿进。
//!
//! 一台 SSH / Telnet / VNC / RDP 主机可以把代理选成"经 EasyTier 组网"
//! （[`crate::proxy::ProxyMode::EasyTier`]），指着一台组网主机。拨号那一刻要
//! 回答三件事，都在这里：
//!
//! 1. **那张网进来了没有。** 组网会话（[`super::session`]）起网时在这里登记一格
//!    （[`Ticket`]），起来了、此刻走得到哪些地址、要停了、出了事，一路报进来。
//! 2. **没进来怎么办。** 托界面去加入它（[`set_launcher`] 装上的那条通道，主机
//!    视图在另一头打开那张网的标签页），然后等。拨号在 tokio 那一侧，手上没有
//!    gpui 的 `App`，自己开不了标签页 —— 只能托人。主机视图打开这种主机时会先
//!    把网加入再开会话，这条通道兜的是之后的每一次重连。
//! 3. **目标到得了没有。** 网进来了不等于目标在线：节点要连上、路由要同步过来。
//!    EasyTier 的 SOCKS5 入口对网里还没有路由的地址不报错，而是退回本机路由
//!    直连去试（它的 `OverlayOrDirect`），一个虚拟 IP 这么试多半卡到超时，报回来
//!    的只是一句笼统的"连不上"。所以目标是 IP 时先等它出现在这张网的路由里
//!    （[`Reach`]：节点的虚拟地址或代理出来的子网），等不到就把话说清楚，不去撞
//!    那一下。目标是域名时不等：那要到 SOCKS5 入口那一头才解析。
//!
//! 登记处是进程级的一张表（和全局代理那份缓存一个道理，见 [`crate::proxy`]），
//! 键是组网主机的 id。每次起网发一个新的 token：上一次连接收尾晚到的那一下只认
//! 自己的 token，抹不掉这一次的登记；等待的一方也靠它分辨一次失败是上一回留下的
//! 遗言，还是这一回的结果。

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use gpui_kit::SharedString;
use rust_i18n::t;
use tokio::sync::watch;
use tokio::time::Instant;

use super::Reach;

/// 等一张网加入最多等多久。起实例本身最多 30 秒（会话那一层的启动超时），
/// 再给界面接手、开标签页留一点。
const JOIN_TIMEOUT: Duration = Duration::from_secs(40);

/// 网进来之后，等目标出现在路由里最多等多久。连上节点、把路由同步过来平常
/// 是几秒的事；二十秒还没有，那台机器多半不在线。
const ROUTE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
enum State {
    /// 在起：建实例、开 SOCKS5 入口 / 虚拟网卡。
    Starting,
    /// 起来了。`socks5` 是它的入口（虚拟网卡那一档没有），`reach` 是此刻
    /// 经它走得到的地址，一秒一更新。
    Running {
        socks5: Option<SocketAddr>,
        reach: Reach,
    },
    /// 起不来、半路死了，或者界面办不成（那台组网主机没了）。带着那句话。
    Failed(SharedString),
}

#[derive(Clone, Debug)]
struct Entry {
    token: u64,
    /// 那台组网主机的名字，报错时说"组网「家里」……"用。
    name: SharedString,
    state: State,
}

static ENTRIES: LazyLock<Mutex<HashMap<SharedString, Entry>>> = LazyLock::new(Default::default);

/// 登记处每变一下就加一。等待的一方订阅它，变了再回来看一眼表。
static CHANGES: LazyLock<watch::Sender<u64>> = LazyLock::new(|| watch::channel(0).0);

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// 替拨号去加入一张网的那条通道，另一头是主机视图。见 [`set_launcher`]。
static LAUNCHER: Mutex<Option<UnboundedSender<SharedString>>> = Mutex::new(None);

/// 表的锁。中毒了照样拿来用：里面是一份状态快照，读到一份可能过时的值远好过
/// 让拨号 panic。
fn entries() -> MutexGuard<'static, HashMap<SharedString, Entry>> {
    ENTRIES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn changed() {
    CHANGES.send_modify(|version| *version = version.wrapping_add(1));
}

fn next_token() -> u64 {
    NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
}

/// 一次起网在登记处的那一格。组网会话攥着它一路报状态，收尾时交回
/// （[`Self::finish`]）；半路掉在地上（会话线程 panic）就当出了事。
///
/// 每个方法都只动**自己那次**登记：表里那一格已经换成了下一次起网的，就什么
/// 也不做。
pub struct Ticket {
    network: SharedString,
    token: u64,
    finished: bool,
}

/// 登记一次起网：`network` 是组网主机的 id，`name` 是它的名字。上一次的登记
/// （连同上一次的失败）一并换掉。
pub fn open(network: SharedString, name: SharedString) -> Ticket {
    let token = next_token();
    entries().insert(
        network.clone(),
        Entry {
            token,
            name,
            state: State::Starting,
        },
    );
    changed();
    Ticket {
        network,
        token,
        finished: false,
    }
}

impl Ticket {
    /// 改自己那一格。`edit` 返回有没有真的改了什么 —— 没改就不去叫醒等待的一方。
    fn update(&self, edit: impl FnOnce(&mut Entry) -> bool) {
        let touched = entries()
            .get_mut(&self.network)
            .filter(|entry| entry.token == self.token)
            .is_some_and(edit);
        if touched {
            changed();
        }
    }

    /// 实例起来了：SOCKS5 入口（虚拟网卡那一档是 `None`）已经在听。
    pub fn running(&self, socks5: Option<SocketAddr>) {
        self.update(|entry| {
            entry.state = State::Running {
                socks5,
                reach: Reach::default(),
            };
            true
        });
    }

    /// 一次采样：此刻走得到哪些地址。和上一次一样就不吵醒谁。
    pub fn reach(&self, reach: Reach) {
        self.update(|entry| match &mut entry.state {
            State::Running { reach: current, .. } if *current != reach => {
                *current = reach;
                true
            }
            _ => false,
        });
    }

    /// 要停了。立刻从表里拿掉：收尾还要一会儿（关监听、拆网卡），这段时间里
    /// 新来的拨号不该再往一张正在拆的网里走。
    pub fn stopping(&self) {
        let removed = {
            let mut entries = entries();
            match entries.get(&self.network) {
                Some(entry) if entry.token == self.token => {
                    entries.remove(&self.network);
                    true
                }
                _ => false,
            }
        };
        if removed {
            changed();
        }
    }

    /// 收尾：`None` 是要它停的（拿掉那一格），`Some` 是出了事（留下那句话，
    /// 等着的拨号拿去说给用户听）。
    pub fn finish(mut self, outcome: Option<SharedString>) {
        self.finished = true;
        self.settle(outcome);
    }

    fn settle(&self, outcome: Option<SharedString>) {
        match outcome {
            None => self.stopping(),
            Some(reason) => self.update(|entry| {
                entry.state = State::Failed(reason);
                true
            }),
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.finished {
            self.settle(Some(t!("easytier.thread_crashed").into()));
        }
    }
}

/// 装上替拨号加入组网的那条通道。主机视图建起来时装一次；再装一次换掉上一条。
/// 通道那一头收到的是组网主机的 id。
pub fn set_launcher(launcher: UnboundedSender<SharedString>) {
    *LAUNCHER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(launcher);
}

/// 托界面去加入这张网。没人接（没装、那一头没了）就是 `false`。
fn ask(network: &SharedString) -> bool {
    LAUNCHER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|launcher| launcher.unbounded_send(network.clone()).is_ok())
}

/// 界面接了请求却办不成（那台组网主机已经删掉了）：登记成出事，等着的拨号当场
/// 拿到这句话，不必干等到超时。
pub fn fail(network: SharedString, reason: SharedString) {
    entries().insert(
        network,
        Entry {
            token: next_token(),
            name: SharedString::default(),
            state: State::Failed(reason),
        },
    );
    changed();
}

/// 经 `network` 那张网去连 `target`（主机名一格的原文）之前，等它就绪。
///
/// 返回怎么进：`Some` 是那张网的 SOCKS5 入口；`None` 是虚拟网卡那一档 ——
/// 系统路由自己会把虚拟 IP 送进网卡，直连就是。
///
/// 网没进来就托界面去加入（见模块文档），最多等 [`JOIN_TIMEOUT`]；进来了再等
/// 目标出现在路由里，最多 [`ROUTE_TIMEOUT`]。起不来、中途退出、等不到，都给
/// 一句能直接摆给用户看的话。
pub async fn wait(network: &str, target: &str) -> Result<Option<SocketAddr>, SharedString> {
    let network = SharedString::from(network.to_string());
    let target_ip = target.trim().parse::<Ipv4Addr>().ok();
    // 先订阅再看表：看完到开始等之间的那一下变化也不会漏。
    let mut changes = CHANGES.subscribe();
    let join_deadline = Instant::now() + JOIN_TIMEOUT;
    let mut route_deadline = None;
    // 已经托过界面了没有；托之前看到的那次失败是上一回的遗言，不算数。
    let mut asked = false;
    let mut stale = None;
    // 这一回见过它在起 / 在跑 —— 之后它从表里没了，就是被人停掉了。
    let mut joined = false;
    let mut name = SharedString::default();

    loop {
        let entry = entries().get(&network).cloned();
        if let Some(entry) = &entry
            && !entry.name.is_empty()
        {
            name = entry.name.clone();
        }
        let deadline = match entry.map(|entry| (entry.token, entry.state)) {
            Some((_, State::Running { socks5, reach })) => {
                joined = true;
                let Some(ip) = target_ip.filter(|ip| !reach.contains(*ip)) else {
                    return Ok(socks5);
                };
                let deadline =
                    *route_deadline.get_or_insert_with(|| Instant::now() + ROUTE_TIMEOUT);
                if Instant::now() >= deadline {
                    return Err(t!(
                        "easytier.portal_no_route",
                        name => name,
                        target => ip,
                        secs => ROUTE_TIMEOUT.as_secs(),
                    )
                    .into());
                }
                deadline
            }
            Some((_, State::Starting)) => {
                joined = true;
                join_deadline
            }
            // 上一回的遗言：已经托界面重新加入了，等这一回的结果。
            Some((token, State::Failed(_))) if stale == Some(token) => join_deadline,
            // 这一回的结果：见过它在起，或者是托界面之后才来的。
            Some((_, State::Failed(reason))) if joined || asked => {
                return Err(failed(&name, &reason));
            }
            // 一上来看到的就是一次失败：那是上一回留下的，托界面重新加入。
            Some((token, State::Failed(reason))) => {
                stale = Some(token);
                if !ask(&network) {
                    return Err(failed(&name, &reason));
                }
                asked = true;
                join_deadline
            }
            None if joined => return Err(t!("easytier.portal_left", name => name).into()),
            None if !asked => {
                if !ask(&network) {
                    return Err(t!("easytier.portal_not_joined").into());
                }
                asked = true;
                join_deadline
            }
            None => join_deadline,
        };

        if Instant::now() >= deadline {
            return Err(match name.is_empty() {
                true => t!("easytier.portal_join_timeout_unnamed", secs => JOIN_TIMEOUT.as_secs()),
                false => t!(
                    "easytier.portal_join_timeout",
                    name => name,
                    secs => JOIN_TIMEOUT.as_secs(),
                ),
            }
            .into());
        }
        tokio::select! {
            _ = changes.changed() => {}
            _ = tokio::time::sleep_until(deadline) => {}
        }
    }
}

/// 这张网没进来，原因是 `reason`。界面办不成的那种（没登记名字）原话就是全部。
fn failed(name: &SharedString, reason: &SharedString) -> SharedString {
    match name.is_empty() {
        true => reason.clone(),
        false => t!("easytier.portal_failed", name => name, reason => reason).into(),
    }
}

/// 测试用：把"这条测试要动托人那条通道"串起来。
///
/// 通道和登记处都是整个进程共用的；登记处按组网 id 分格，各条测试用各自的 id
/// 就互不相扰，通道却只有一条。拿着这把锁的测试可以随便装、随便拆，守卫掉地时
/// 把通道拆掉，panic 了也一样。
#[cfg(test)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: Mutex<()> = Mutex::new(());

    struct Restore(#[allow(dead_code)] MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            *LAUNCHER
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    }
    let guard = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *LAUNCHER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    Restore(guard)
}

#[cfg(test)]
mod tests {
    use futures::StreamExt as _;

    use super::*;
    use crate::easytier::NetworkSnapshot;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// 一份走得到 `ips` 的采样。
    fn reach(ips: &[&str]) -> Reach {
        let mut snapshot = NetworkSnapshot::default();
        snapshot.peers = ips
            .iter()
            .map(|ip| crate::easytier::PeerNode {
                peer_id: 0,
                hostname: "".into(),
                ipv4: Some(ip.to_string().into()),
                path: crate::easytier::PeerPath::Direct,
                latency_ms: None,
                loss_rate: None,
                rx_bytes: 0,
                tx_bytes: 0,
                tunnels: Vec::new(),
                nat: Default::default(),
                version: "".into(),
                public_server: false,
                subnets: Vec::new(),
            })
            .collect();
        snapshot.reach()
    }

    fn socks5() -> SocketAddr {
        "127.0.0.1:11080".parse().unwrap()
    }

    /// 最常见的那一条：网在起，拨号先等着；起来了、目标的路由也到了，拿到
    /// SOCKS5 入口。路由没到之前那几拍不放行 —— 那时候入口会退回直连去撞。
    #[test]
    fn a_dial_waits_for_the_network_and_the_route() {
        let ticket = open("portal-test-route".into(), "家里".into());
        runtime().block_on(async {
            let waiting = tokio::spawn(wait("portal-test-route", " 10.144.144.5 "));
            tokio::task::yield_now().await;
            ticket.running(Some(socks5()));
            ticket.reach(reach(&["10.144.144.2"]));
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(!waiting.is_finished(), "目标的路由还没到");

            ticket.reach(reach(&["10.144.144.2", "10.144.144.5"]));
            let via = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .expect("路由到了就该放行")
                .unwrap();
            assert_eq!(via, Ok(Some(socks5())));
        });
        ticket.finish(None);
    }

    /// 虚拟网卡那一档没有入口：放行的是"直连"；目标是域名时不等路由。
    #[test]
    fn a_tun_network_is_a_direct_connection_and_names_skip_the_route() {
        let ticket = open("portal-test-tun".into(), "机房".into());
        ticket.running(None);
        let via = runtime().block_on(wait("portal-test-tun", "db.internal"));
        assert_eq!(via, Ok(None));
        ticket.finish(None);
    }

    /// 这一回起网失败：原因带着网的名字原样交回，不等到超时。
    #[test]
    fn a_failed_start_is_reported_with_its_reason() {
        let _serial = exclusive_for_test();
        let ticket = open("portal-test-failed".into(), "家里".into());
        runtime().block_on(async {
            let waiting = tokio::spawn(wait("portal-test-failed", "10.144.144.5"));
            tokio::task::yield_now().await;
            ticket.finish(Some("端口被占了".into()));
            let error = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .expect("失败了就该当场说")
                .unwrap()
                .expect_err("网没起来");
            assert!(error.contains("家里") && error.contains("端口被占了"), "{error}");
        });
    }

    /// 网没进来、也没人能替它加入（主机视图不在）：当场说清楚。
    #[test]
    fn nobody_to_join_the_network_is_an_error_right_away() {
        let _serial = exclusive_for_test();
        let error = runtime()
            .block_on(wait("portal-test-nobody", "10.144.144.5"))
            .expect_err("没进来");
        assert_eq!(error, SharedString::from(t!("easytier.portal_not_joined")));
    }

    /// 网没进来：托界面去加入，等界面那一头起网；上一回留下的失败是遗言，
    /// 不当这一回的结果。
    #[test]
    fn an_absent_network_is_joined_through_the_launcher() {
        let _serial = exclusive_for_test();
        const NETWORK: &str = "portal-test-launcher";
        // 上一回失败留下的那一格。
        open(NETWORK.into(), "家里".into()).finish(Some("上一回的".into()));

        let (launcher, mut requests) = futures::channel::mpsc::unbounded();
        set_launcher(launcher);
        runtime().block_on(async {
            let waiting = tokio::spawn(wait(NETWORK, "10.144.144.5"));
            let requested = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("该托界面去加入");
            assert_eq!(requested.as_deref(), Some(NETWORK));

            // 界面那一头：起网，起来了，路由到了。
            let ticket = open(NETWORK.into(), "家里".into());
            ticket.running(Some(socks5()));
            ticket.reach(reach(&["10.144.144.5"]));
            let via = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .expect("起来了就该放行")
                .unwrap();
            assert_eq!(via, Ok(Some(socks5())));
            ticket.finish(None);
        });
    }

    /// 界面办不成（那台组网主机删掉了）：它登记的那句话直接交回。
    #[test]
    fn a_launcher_that_cannot_help_says_why() {
        let _serial = exclusive_for_test();
        const NETWORK: &str = "portal-test-missing";
        let (launcher, mut requests) = futures::channel::mpsc::unbounded();
        set_launcher(launcher);
        runtime().block_on(async {
            let waiting = tokio::spawn(wait(NETWORK, "10.144.144.5"));
            let requested = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .unwrap();
            fail(requested.unwrap(), "那台组网主机没了".into());
            let error = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .unwrap()
                .unwrap()
                .expect_err("办不成");
            assert_eq!(error, SharedString::from("那台组网主机没了"));
        });
    }

    /// 等着的时候网被人停了：说它退出了，不再托界面把它拉起来 —— 那是用户
    /// 刚刚明确做的决定。
    #[test]
    fn a_network_stopped_while_waiting_is_not_rejoined() {
        let ticket = open("portal-test-left".into(), "家里".into());
        ticket.running(Some(socks5()));
        runtime().block_on(async {
            let waiting = tokio::spawn(wait("portal-test-left", "10.144.144.5"));
            tokio::task::yield_now().await;
            ticket.stopping();
            let error = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .unwrap()
                .unwrap()
                .expect_err("网退出了");
            assert_eq!(
                error,
                SharedString::from(t!("easytier.portal_left", name => "家里"))
            );
        });
        ticket.finish(None);
    }

    /// 上一次连接收尾晚到的那一下抹不掉这一次的登记。
    #[test]
    fn a_late_goodbye_from_an_older_start_is_ignored() {
        let older = open("portal-test-late".into(), "家里".into());
        let newer = open("portal-test-late".into(), "家里".into());
        newer.running(Some(socks5()));
        older.finish(Some("旧的".into()));
        let via = runtime().block_on(wait("portal-test-late", "db.internal"));
        assert_eq!(via, Ok(Some(socks5())), "旧的那次抹不掉这一次");
        newer.finish(None);
        assert!(entries().get("portal-test-late").is_none(), "停了就拿掉");
    }

    /// 会话线程没交回那一格就没了（panic）：当它出了事，别让拨号干等到超时。
    #[test]
    fn a_dropped_ticket_counts_as_a_crash() {
        drop(open("portal-test-crash".into(), "家里".into()));
        let state = entries().get("portal-test-crash").map(|entry| entry.state.clone());
        assert!(
            matches!(&state, Some(State::Failed(reason)) if reason.as_ref() == t!("easytier.thread_crashed")),
            "{state:?}"
        );
    }
}
