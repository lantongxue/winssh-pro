//! 运行中的那个 mihomo 实例：起停、端口、日志，以及界面订阅的那份状态。
//!
//! **本进程里同时只有一个实例。** 系统代理是整台机器一份的，虚拟网卡也是
//! 一块，两个实例并存没有意义，只会互相抢端口、抢网卡。所以这里没有"每张网
//! 一个会话"那套（对比 [`crate::easytier::session`]），只有进程级的一格：
//! 谁在跑、跑的是哪条订阅、它此刻什么状态。
//!
//! ## 一扇线程 + 一个自己的 runtime
//!
//! 和组网那边同一个理由（见 `crate::easytier::session` 的模块文档）：这一档要
//! 看住一个**子进程**，还要每两秒问它一次话。这些活儿的寿命该和这次运行绑在
//! 一起 —— 停了就整条 runtime 拆掉，而不是留在数据层那个 runtime 上继续跑。
//!
//! ## 状态怎么到界面上去
//!
//! 一条进程级的 [`watch`] 通道（[`state`] / [`subscribe`]），谁想看谁订阅，任何
//! 时候订阅都先拿到**此刻**那一份。这和组网那边的 mpsc 通道有个要紧的区别：
//! 面板关掉再打开，读到的是现在，而不是"从关掉那一刻起攒下的变化" —— 这一档
//! 本来就**不跟着面板的生死走**：把面板关了，代理照跑。
//!
//! ## 收尾
//!
//! 三种收尾（用户点停、子进程自己没了、应用退出）都要做完同样两件事：把子
//! 进程收掉，把系统代理**还原**。第三种的收尾来不及走这条线程（应用正在退
//! 出），所以 [`shutdown`] 自己动手，见那个函数的文档。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::SharedString;
use rust_i18n::t;
use tokio::sync::{mpsc, watch};

use serde_json::Value;

use super::api::ApiClient;
use super::config::{self, RuntimePorts, SubscriptionOutline};
use super::snapshot::{ConnectionSummary, LiveState, NodeFacts};
use super::subscription::ClashSubscription;

/// 起来了之后，多久问一次内核"现在什么样"。
///
/// 两秒是"节点切换与测速结果够快地反映到界面上"与"别拿一条本机 HTTP 每秒
/// 敲一次"之间的那一档。问的是 `/proxies` 这一份（节点表，几十 KB），
/// 规则那份只在起来时与手动刷新时取 —— 它不会自己变。
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// 起进程之后，等控制口答话最多等多久。
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// 等控制口时每轮之间歇多久。第一轮之前先歇一下：进程刚 fork 出来，
/// 那一下连接必然是拒的。
const READY_INTERVAL: Duration = Duration::from_millis(400);

/// 拆这个 runtime 最多给多久。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// 日志留多少行。日志是给"起不来的时候看看到底怎么了"用的，不是给人读的，
/// 留一条尾巴就够。
const LOG_LIMIT: usize = 120;

/// 启动那一段单独留一份：起来之后内核的日志会把它冲出尾巴，
/// 而排"为什么起不来"要看的正是那一段。
const STARTUP_LIMIT: usize = 200;

/// 报错时带上启动日志的最后几行 —— 原因常常就在那几行里。
const REASON_LINES: usize = 6;

/// 界面可以下的那几道指令。
#[derive(Clone, Debug)]
pub enum Action {
    /// 收掉这次运行。
    Stop,
    /// 拨/收虚拟网卡（TUN）。由内核建网卡，我们只拨它的开关。
    SetTun(bool),
    /// 拨/收系统代理。
    SetSystemProxy(bool),
    /// 换分流模式：`rule` / `global` / `direct`。
    SetMode(SharedString),
    /// 把某个策略组切到 `node` 上。
    Select {
        group: SharedString,
        node: SharedString,
    },
    /// 把一个组里的节点测一遍速。
    TestDelay(SharedString),
    /// 只测一个节点。列表里那一行自己那颗按钮走的是它。
    TestNodeDelay(SharedString),
    /// 立刻重采一次（手动刷新）。
    Refresh,
    /// 一轮测速在独立任务里跑完了：重采一次节点表，并把进行中的测速计数减一。
    /// 是内部指令，界面不该发它。
    DelayTestDone,
    /// 测试正文里能看到的全部策略组。
    TestDelayAll,
    /// 关闭一条实时连接。
    CloseConnection(SharedString),
    /// 关闭全部实时连接。
    CloseConnections,
    /// 清空界面上的内核日志尾巴。
    ClearLogs,
    /// 正文换了：重写配置文件并让内核重读一遍，不重起进程。
    Reload(SharedString),
}

/// 这次运行走到哪一步了。
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// 没有实例在跑。
    Idle,
    /// 在起：写配置、起进程、等控制口答话。
    Starting,
    /// 在跑。
    Running,
    /// 要停：子进程正在收、系统代理正在还原。
    Stopping,
    /// 起不来，或者半路死了。带着原因。
    Failed(SharedString),
}

impl Phase {
    /// 子进程此刻是不是（或者应该是）活着的。界面拿它决定按钮是"启动"还是
    /// "停止"。
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

/// 在跑的是哪条订阅。只有 id 与名字 —— 正文与节点表都不在这儿，
/// 那些要么在库里，要么在 [`RuntimeState::live`] 里。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscriptionRef {
    pub id: SharedString,
    pub name: SharedString,
}

/// 一次没办成的动作。带一个序号，界面按序号决定要不要弹通知 ——
/// 不必回送一句"我弹过了"。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorReport {
    pub seq: u64,
    pub message: SharedString,
}

/// 此刻的样子。界面每一帧读的就是它。
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeState {
    pub phase: Phase,
    /// 在跑（或者刚跑过）的那条订阅。
    pub subscription: Option<SubscriptionRef>,
    /// 混合端口：HTTP 与 SOCKS 共用它，系统代理指的就是它。
    pub mixed_port: Option<u16>,
    /// 外部控制口。
    pub api_port: Option<u16>,
    /// 分流模式。
    pub mode: SharedString,
    /// 系统代理**此刻是不是我们设的**（不是"用户想不想"）。
    pub system_proxy: bool,
    /// 虚拟网卡此刻是不是开着。
    pub tun: bool,
    /// 内核版本，起来了才有。
    pub version: SharedString,
    /// 节点与规则。起来了才有。
    pub live: Option<LiveState>,
    /// 此刻的内核吞吐，`"1.2 MB"` / `"512 KB"` 这种现成的话。起来才有。
    pub traffic_up: SharedString,
    /// 同上，下行。
    pub traffic_down: SharedString,
    /// 子进程日志的尾巴，最后一行在最后。
    pub logs: Vec<SharedString>,
    /// 当前活跃连接。
    pub connections: Vec<ConnectionSummary>,
    /// 此刻还在跑的测速轮数。界面拿它决定"测速中"那个状态何时撤掉 ——
    /// 测速在独立任务里跑，不再占着会话循环，不能靠"状态变了"来猜它完没完。
    pub delay_tests: u32,
    /// 最近一次没办成的事。
    pub error: Option<ErrorReport>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            subscription: None,
            mixed_port: None,
            api_port: None,
            mode: "rule".into(),
            system_proxy: false,
            tun: false,
            version: SharedString::default(),
            live: None,
            traffic_up: SharedString::default(),
            traffic_down: SharedString::default(),
            logs: Vec::new(),
            connections: Vec::new(),
            delay_tests: 0,
            error: None,
        }
    }
}

impl RuntimeState {
    /// 这条订阅此刻正在跑吗。面板拿它决定摆"启动"还是"停止"。
    pub fn is_current(&self, id: &str) -> bool {
        self.subscription
            .as_ref()
            .is_some_and(|running| running.id == id)
    }

    /// 跑着的这条是不是"这条订阅、且确实活着"。
    pub fn is_live(&self, id: &str) -> bool {
        self.is_current(id) && self.phase.is_busy()
    }
}

/// 此刻的样子。面板开起来先读一次它。
pub fn state() -> Arc<RuntimeState> {
    let current = snapshot();
    Arc::clone(&current)
}

/// 订阅变化。拿到手先读一次值，再等 `changed()` —— 中间那一下不会漏。
pub fn subscribe() -> watch::Receiver<Arc<RuntimeState>> {
    CHANGES.subscribe()
}

/// 起一条订阅。已经在跑的先收掉。
///
/// 这里只做两件**当场能判定**的事：找不找得到 mihomo、有没有别的实例在跑。
/// 写配置、起进程、等控制口都归会话线程 —— 那几件事要时间，界面不该在这儿
/// 卡着；成不成，看 [`Phase`] 怎么走。
pub fn start(subscription: &ClashSubscription) -> Result<()> {
    if subscription.config_yaml.trim().is_empty() {
        bail!(t!("clash.runtime.empty_config"));
    }
    if binary_path().is_err() {
        bail!(t!("clash.runtime.no_binary", name => MIHOMO_EXE));
    }
    // 换一条订阅就是换一个端口，而系统代理指的是**上一个实例那个口** ——
    // 不还回去的话，那个口马上没人听，用户的网就断在那儿。所以开新的之前
    // 先把系统代理收掉；开关也跟着回到"关"，用户在新实例上再拨一次，
    // 拨的是新的端口。这里不静默地把代理指过去：改本机网络设置这件事，
    // 得是用户看着一个开关做的。
    if state().system_proxy
        && let Err(error) = super::system_proxy::disable()
    {
        log::warn!("换实例前还原系统代理失败：{error:#}");
    }
    // 已经在跑的（哪怕是同一条）先收掉：一个实例换一次订阅，收干净再起。
    stop_and_wait();

    let token = next_token();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let stop = Arc::new(tokio::sync::Notify::new());
    *current() = Some(Running {
        input: input_tx.clone(),
        stop: stop.clone(),
        pid: None,
        token,
    });
    publish(|state| {
        state.phase = Phase::Starting;
        state.subscription = Some(SubscriptionRef {
            id: subscription.id.clone(),
            name: subscription.name.clone(),
        });
        state.mixed_port = None;
        state.api_port = None;
        state.version = SharedString::default();
        state.live = None;
        state.connections.clear();
        state.logs.clear();
        state.system_proxy = false;
        state.tun = false;
        state.delay_tests = 0;
        state.traffic_up = SharedString::default();
        state.traffic_down = SharedString::default();
    });

    let subscription = subscription.clone();
    std::thread::Builder::new()
        .name("clash-mihomo".to_owned())
        .spawn(move || session(subscription, token, input_tx, input_rx, stop))
        .map_err(|error| anyhow!("启动 mihomo 会话线程失败：{error}"))?;
    Ok(())
}

/// 收掉。没在跑就什么也不做。
///
/// 先摇**抢占铃**再排 [`Action::Stop`]：会话线程不管此刻正卡在哪一步都会被
/// 当场打断去收尾；界面也立刻看到"正在停止"，不必等线程醒过来。
pub fn stop() {
    let target = current()
        .as_ref()
        .map(|running| (running.input.clone(), running.stop.clone()));
    if let Some((input, stop)) = target {
        stop.notify_one();
        let _ = input.send(Action::Stop);
        stopping();
    }
}

/// 给运行中的实例下一道指令。没在跑就是 `false`。
pub fn send(action: Action) -> bool {
    current()
        .as_ref()
        .is_some_and(|running| running.input.send(action).is_ok())
}

/// 清掉界面保留的日志尾巴。实例已经结束时没有会话线程可接收
/// [`Action::ClearLogs`]，因此由这里直接清快照。
pub fn clear_logs() {
    publish(|state| state.logs.clear());
}

/// 应用退出时的收尾。
///
/// 这条路径上**来不及**等会话线程自己收（应用这一帧就没了）。两件事都得当场
/// 做完，而且顺序不能反：
///
/// 1. **先把系统代理还原。** 留着的话，应用没了、代理口也没了，用户整台机器
///    的网就断在一个没人听的口上 —— 这是他第二天开机才发现的那种坏法。
/// 2. **再把子进程收掉。** 它是我们的孩子，我们没了它就成了孤儿：网卡还在、
///    端口还占着、系统代理（如果没做第 1 步）还指着它。
///
/// 子进程收不掉也不算错：下一次启动的 [`super::system_proxy::reconcile`] 会
/// 发现那个口还活着，把网留给它。
pub fn shutdown() {
    if state().system_proxy
        && let Err(error) = super::system_proxy::disable()
    {
        log::warn!("退出时还原系统代理失败：{error:#}");
    }
    let pid = current().as_ref().and_then(|running| running.pid);
    if let Some(pid) = pid {
        kill_pid(pid);
    }
}

/// 这条订阅正跑着吗。别的地方（比如设置页）想知道"代理开着没有"时用它。
pub fn running_subscription() -> Option<SubscriptionRef> {
    let state = state();
    match state.phase.is_busy() {
        true => state.subscription.clone(),
        false => None,
    }
}

/// 实例的数据目录：`db::db_dir()/clash/instances/<订阅 id>`。
///
/// 一条订阅一个目录，**不随每次启动换**：mihomo 把"某个策略组上次选中了哪个
/// 节点"记在它自己的 `cache.db` 里，换目录就等于每次启动都把它忘掉。
pub fn instance_dir(subscription_id: &str) -> Result<PathBuf> {
    let dir = clash_dir()?.join("instances").join(safe_name(subscription_id));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("建 mihomo 数据目录失败：{}", dir.display()))?;
    Ok(dir)
}

/// 把一条订阅的运行痕迹扫掉：它的数据目录（那份现写的 `config.yaml`、内核的
/// `cache.db` 与 geo 数据）。
///
/// 订阅删掉之后叫的，所以**尽力而为**：删不掉只记一行日志 —— 那个目录里没有
/// 任何别处读的东西，留着只是占点地方，不值得为它弹一句通知。
pub fn forget(subscription_id: &str) {
    let dir = match clash_dir() {
        Ok(dir) => dir.join("instances").join(safe_name(subscription_id)),
        Err(error) => {
            log::warn!("找不到 Clash 数据目录，运行痕迹没扫掉：{error:#}");
            return;
        }
    };
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => log::info!("已扫掉 {} 的运行痕迹", dir.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("扫 {} 失败：{error}", dir.display()),
    }
}

/// Clash 这一档在用户数据目录里的那一块：`db::db_dir()/clash`。
///
/// 里面是实例的数据目录与系统代理那份复原记录（见
/// [`super::system_proxy`]）。都是**这台机器的事**，不进库、不跟着备份走。
pub fn clash_dir() -> Result<PathBuf> {
    let dir = crate::db::db_dir()?.join("clash");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("建 Clash 数据目录失败：{}", dir.display()))?;
    Ok(dir)
}

/// mihomo 那个可执行文件在哪儿。
///
/// 排在前面的两处是**发出去的样子**：构建脚本把它拷到 exe 旁边
/// （`build.rs` 的 `copy_sidecar`），`cargo test` 起的测试 exe 在 `deps/` 里，
/// 所以父目录也看一眼。最后那条是开发时的原处 —— 只有 debug 构建才认，
/// 发布出去的 exe 旁边那一行 `CARGO_MANIFEST_DIR` 指的是构建机的目录，
/// 拿它去找文件等于碰运气。
pub fn binary_path() -> Result<PathBuf> {
    let name = MIHOMO_EXE;
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for candidate in [dir.join(name), dir.join("..").join(name)] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    if let Ok(dir) = clash_dir() {
        for candidate in [dir.join("bin").join(name), dir.join(name)] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    #[cfg(debug_assertions)]
    for relative in DEV_CANDIDATES {
        let candidate = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    bail!(t!("clash.runtime.no_binary", name => name));
}

#[cfg(windows)]
pub const MIHOMO_EXE: &str = "mihomo.exe";
#[cfg(not(windows))]
pub const MIHOMO_EXE: &str = "mihomo";

/// 开发时到哪儿去找那份二进制（用户自己放进来的，见 `third_party/mihomo/`）。
#[cfg(debug_assertions)]
const DEV_CANDIDATES: [&str; 3] = [
    "third_party/mihomo/windows/mihomo-windows-amd64.exe",
    "third_party/mihomo/mihomo.exe",
    "third_party/mihomo/mihomo",
];

/// 在跑的那一格。`pid` 是给 [`shutdown`] 用的 —— 那条路径上够不着 tokio 的
/// `Child`，只能记下号来从外面收。
///
/// `token` 是这一格的号牌：每一次 [`start`] 发一张新的。收尾晚到的那几拍
/// （子进程迟迟不肯死、runtime 拆得慢）凭它认出"台上已经不是我了"，
/// 于是不去动那一格、也不去写那份状态 —— 否则一次慢收尾会把**新**实例的
/// 输入通道抹成空，那颗"停止"从此按不动。
struct Running {
    input: mpsc::UnboundedSender<Action>,
    /// 点停时**抢占**用的铃：会话线程在等的任何一件事（下载、等控制口、
    /// 一轮采样、一条慢指令）都会被它打断。光靠 `input` 里排一条
    /// [`Action::Stop`] 不够 —— 队伍前面有活儿在跑的话，它要等到那件事做完
    /// 才轮得到。
    stop: Arc<tokio::sync::Notify>,
    pid: Option<u32>,
    token: u64,
}

/// 进程级那一格，与它上面那把锁。
static CURRENT: LazyLock<Mutex<Option<Running>>> = LazyLock::new(Default::default);

/// 界面订阅的那条通道。整个进程共用一条。
static CHANGES: LazyLock<watch::Sender<Arc<RuntimeState>>> =
    LazyLock::new(|| watch::channel(Arc::new(RuntimeState::default())).0);

/// 此刻那一份。写的人先改它，再往通道上发一份。
static SNAPSHOT: LazyLock<Mutex<Arc<RuntimeState>>> =
    LazyLock::new(|| Mutex::new(Arc::new(RuntimeState::default())));

static ERROR_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 发号牌的那一支。见 [`Running::token`]。
static NEXT_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_token() -> u64 {
    NEXT_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// 台上这一格还是 `token` 那一次运行的吗。
fn same_run(token: u64) -> bool {
    current().as_ref().is_some_and(|running| running.token == token)
}

fn current() -> MutexGuard<'static, Option<Running>> {
    CURRENT.lock().unwrap_or_else(poisoned)
}

fn snapshot() -> MutexGuard<'static, Arc<RuntimeState>> {
    SNAPSHOT.lock().unwrap_or_else(poisoned)
}

/// 锁中毒了照样拿来用：里面是一份状态快照，读到一份可能过时的值远好过
/// 让整个面板打不开。
fn poisoned<T>(error: std::sync::PoisonError<T>) -> T {
    error.into_inner()
}

/// 改一处状态，然后把新的一份发出去。
fn publish(edit: impl FnOnce(&mut RuntimeState)) {
    let next = {
        let mut guard = snapshot();
        let mut state = RuntimeState::clone(&guard);
        edit(&mut state);
        let next = Arc::new(state);
        *guard = next.clone();
        next
    };
    // 没有订阅者（界面没开）时这一下会失败，那不算错。
    let _ = CHANGES.send(next);
}

/// 记下"正在收"。
///
/// 收尾不是一瞬间的事（子进程要收、TUN 网卡要拆、系统代理要还原），这一段
/// 时间里界面得说清楚在干什么 —— 否则那颗按钮按下去像没反应。
fn stopping() {
    publish(|state| state.phase = Phase::Stopping);
}

/// 报一次错：记下来，界面看到序号变了就弹一次通知。
fn report(message: impl Into<SharedString>) {
    let seq = ERROR_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    publish(|state| {
        state.error = Some(ErrorReport {
            seq,
            message: message.into(),
        })
    });
}

/// 把当前这次运行收掉并**等它真的收完**（最多等一秒）。
///
/// 起新实例之前必须等：上一个还攥着网卡和端口的话，新的那个起来就得跟它抢。
fn stop_and_wait() {
    if current().is_none() {
        return;
    }
    stop();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while current().is_some() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 会话线程：起进程、看住它、收尾。
fn session(
    subscription: ClashSubscription,
    token: u64,
    input_tx: mpsc::UnboundedSender<Action>,
    input_rx: mpsc::UnboundedReceiver<Action>,
    stop: Arc<tokio::sync::Notify>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            finish(token, Some(format!("建 mihomo 会话 runtime 失败：{error}")));
            return;
        }
    };
    let outcome = runtime.block_on(run(&subscription, token, input_tx, input_rx, stop));
    runtime.shutdown_timeout(SHUTDOWN_GRACE);
    finish(token, outcome);
}

/// 收尾：把这一格清掉，把系统代理还原，再把结果写到状态里。
///
/// 系统代理的还原放在这儿而不是 `run` 里面：无论这次是正常停、起不来还是
/// 半路死了，只要**是我们设的**就得还回去。
///
/// 号牌对不上的话**一根手指都不许动**：台上已经换成下一次运行了，
/// 那一格与那份状态都归它 —— 见 [`Running::token`]。
fn finish(token: u64, outcome: Option<String>) {
    if !same_run(token) {
        log::info!("上一次运行的收尾晚到了，台上已经换人，跳过");
        return;
    }
    let was_proxied = state().system_proxy;
    if was_proxied && let Err(error) = super::system_proxy::disable() {
        log::warn!("还原系统代理失败：{error:#}");
    }
    *current() = None;
    publish(|state| {
        state.system_proxy = false;
        state.tun = false;
        state.delay_tests = 0;
        state.traffic_up = SharedString::default();
        state.traffic_down = SharedString::default();
        state.phase = match outcome {
            Some(reason) => Phase::Failed(reason.into()),
            None => Phase::Idle,
        };
        if state.phase == Phase::Idle {
            state.subscription = None;
            state.mixed_port = None;
            state.api_port = None;
            state.live = None;
            state.connections.clear();
        }
    });
}

/// 会话主体。返回 `Some(原因)` 就是出了事。
async fn run(
    subscription: &ClashSubscription,
    _token: u64,
    input_tx: mpsc::UnboundedSender<Action>,
    mut input_rx: mpsc::UnboundedReceiver<Action>,
    stop: Arc<tokio::sync::Notify>,
) -> Option<String> {
    // 检查并准备 mihomo 可执行文件（若本地未找到则按系统架构自动下载并解压）
    let binary = match binary_path() {
        Ok(path) => path,
        Err(err) => {
            log::error!("未找到 mihomo 内核：{err:#}");
            return Some(format!("{err:#}"));
        }
    };
    log::info!("启动 mihomo：{}", binary.display());
    let dir = match instance_dir(&subscription.id) {
        Ok(dir) => dir,
        Err(error) => return Some(format!("{error:#}")),
    };

    let ports = RuntimePorts {
        mixed_port: free_port()?,
        api_port: free_port()?,
        secret: random_secret(),
    };
    let config_path = dir.join("config.yaml");
    let text = match config::runtime_config(&subscription.config_yaml, &ports) {
        Ok(text) => text,
        Err(error) => return Some(format!("这一份订阅拼不出配置：{error:#}")),
    };
    if let Err(error) = std::fs::write(&config_path, &text) {
        return Some(format!("写 {} 失败：{error}", config_path.display()));
    }

    let api = Arc::new(ApiClient::new(ports.api_port, &ports.secret));
    publish(|state| {
        state.mixed_port = Some(ports.mixed_port);
        state.api_port = Some(ports.api_port);
    });

    // 子进程：`-d` 是它的数据目录（cache.db、geo 数据都落这儿），`-f` 是那份
    // 现写的配置。`CREATE_NO_WINDOW` 是必须的 —— 少了它，一个黑框会跟着弹出来，
    // 而且会一直挂在那儿（见 `crate::service::local_file_service` 里同一处讲究）。
    let mut command = tokio::process::Command::new(&binary);
    command
        .arg("-d")
        .arg(&dir)
        .arg("-f")
        .arg(&config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    #[cfg(windows)]
    {
        // `creation_flags` 这一条是 `tokio::process::Command` 自己的方法，
        // 不必把 `std::os::windows::process::CommandExt` 引进来（那个 trait
        // 是给同步的 `std::process::Command` 用的，见 [`kill_pid`]）。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Some(format!("起 mihomo 失败（{}）：{error}", binary.display()));
        }
    };
    let pid = child.id();
    if let Some(running) = current().as_mut() {
        running.pid = pid;
    }

    let logs = Logs::new();
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(read_lines(stdout, logs.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(read_lines(stderr, logs.clone()));
    }

    // 正文里那份顺序：界面照它摆节点与策略组（见 [`Session::publish_live`]）。
    let outline = config::outline(&subscription.config_yaml).ok();

    // 实时吞吐是条永不收尾的流，单独一个任务消费（见 [`ApiClient::traffic_stream`]），
    // 不占会话循环。
    let traffic_task = tokio::spawn(stream_traffic(api.clone()));

    // `biased`：抢占铃优先。点了停止就不再等任何一件在飞的事，
    // supervise 的 future 被丢掉（里面在等的 HTTP 随之取消），直接去收子进程。
    let outcome = tokio::select! {
        biased;
        _ = stop.notified() => {
            stopping();
            None
        }
        outcome = supervise(&mut child, api.clone(), &dir, ports, logs, outline, input_tx, input_rx) => outcome,
    };
    traffic_task.abort();

    // 收尾：子进程先收掉。`kill` 在它已经退出时是空操作。
    let _ = child.kill().await;
    let _ = child.wait().await;
    if let Some(running) = current().as_mut() {
        running.pid = None;
    }
    outcome
}

/// 在独立任务里把内核的吞吐流一份份发布到状态里。流断了（内核还没起来、
/// 被重载）就隔一秒重连，直到任务被中止。
async fn stream_traffic(api: Arc<ApiClient>) {
    loop {
        if let Err(error) = api.traffic_stream(publish_traffic).await {
            log::debug!("吞吐流断了：{error:#}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// 采一份实时吞吐（上行 / 下行，字节每秒）写进状态。
///
/// 和上一份一样就不发：流每秒一份，而每次发布都要克隆整份状态
/// （节点表、连接表都在里面），没变化的时候不值得。
fn publish_traffic(traffic: Value) {
    let up: SharedString = traffic
        .get("up")
        .and_then(Value::as_u64)
        .map(format_bytes_per_sec)
        .unwrap_or_default()
        .into();
    let down: SharedString = traffic
        .get("down")
        .and_then(Value::as_u64)
        .map(format_bytes_per_sec)
        .unwrap_or_default()
        .into();
    {
        let current = state();
        if current.traffic_up == up && current.traffic_down == down {
            return;
        }
    }
    publish(|state| {
        state.traffic_up = up;
        state.traffic_down = down;
    });
}

/// 一次运行里那几样不随采样变的东西：控制口、数据目录、端口、日志，外加正文
/// 解读出来的那份**顺序**（见 [`Session::publish_live`]）。
struct Session {
    api: Arc<ApiClient>,
    /// 会话自己的指令入口：独立任务（测速）跑完后往里回送指令。
    input: mpsc::UnboundedSender<Action>,
    dir: PathBuf,
    ports: RuntimePorts,
    logs: Logs,
    /// 正文里节点与策略组是按什么顺序写的。内核报回来的是 Go 的 map，
    /// 每次顺序都不一样，摆到界面上会自己跳位置 —— 有这份就照着它摆。
    outline: Option<SubscriptionOutline>,
}

impl Session {
    /// 采一份节点表写进状态里。
    ///
    /// `rules` 给 `None` 时留着上一份：规则不会自己变，只在起来时、手动刷新时
    /// 与换正文之后才重新取，其余那些采样（每两秒一次）不该把那一栏擦成空白。
    fn publish_live(&self, nodes: &Value, rules: Option<&Value>, mode: Option<String>) {
        let mut live = LiveState::from_api(nodes, rules);
        let facts = NodeFacts::from_api(nodes);
        for group in &mut live.groups {
            group.fill(&facts);
        }
        if rules.is_none()
            && let Some(previous) = &state().live
        {
            live.rules = previous.rules.clone();
        }
        if let Some(outline) = &self.outline {
            live = live.ordered_by(outline);
        }

        let logs = self.logs.tail();
        publish(|state| {
            state.live = Some(live);
            state.logs = logs;
            if let Some(mode) = mode {
                state.mode = mode.into();
            }
        });
    }

    /// 采一份连接表。连接不是节点表的一部分，单独发布可以在连接变化时
    /// 更新详情页而不重建节点行模型。
    fn publish_connections(&self, value: &Value) {
        let connections = value
            .get("connections")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(ConnectionSummary::from_entry)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        publish(|state| state.connections = connections);
    }

    fn clear_logs(&self) {
        self.logs.clear();
        publish(|state| state.logs.clear());
    }
}

/// 字节每秒 → 一句短话（"1.2 MB/s"、"88 KB/s"）。EB 那一档只是占格子 ——
/// 一个 mihomo 这辈子吃不到那么多。与 [`format_bytes_per_sec`] 配套的那条测试
/// 把这五档钉住：不要在它们之间再插新档位。
fn format_bytes_per_sec(bytes: u64) -> String {
    let kb = 1024u64;
    let mb = kb * 1024;
    let gb = mb * 1024;
    let tb = gb * 1024;
    let units = [("TB/s", tb), ("GB/s", gb), ("MB/s", mb), ("KB/s", kb)];
    for (label, quantum) in units {
        if bytes >= quantum {
            return format!("{:.1} {label}", bytes as f64 / quantum as f64);
        }
    }
    format!("{bytes} B/s")
}

/// 测速打的那个地址。一个必定回 `204` 的极简页面，Clash 系客户端测速都用它：
/// 它不传输正文，所以量出来的就是纯粹的一次往返。
const TEST_URL: &str = "http://www.gstatic.com/generate_204";

/// 一个节点测速最多等多久。
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// 起来了之后的那一段：等控制口、然后一面向界面报状态、一面听指令。
async fn supervise(
    child: &mut tokio::process::Child,
    api: Arc<ApiClient>,
    dir: &Path,
    ports: RuntimePorts,
    logs: Logs,
    outline: Option<SubscriptionOutline>,
    input_tx: mpsc::UnboundedSender<Action>,
    mut input_rx: mpsc::UnboundedReceiver<Action>,
) -> Option<String> {
    let mut session = Session {
        api,
        input: input_tx,
        dir: dir.to_path_buf(),
        ports,
        logs,
        outline,
    };
    // 起来之前那一段自己拿一份：控制口与日志都是廉价克隆，
    // 之后要把 `session` 借出去（`act` 要 `&mut`），这里不留着借用。
    let api = session.api.clone();
    let logs = session.logs.clone();

    // 等控制口答话。这段时间里进程可能已经死了（配置读不动、端口被占、
    // geo 数据下载不下来……），所以每一轮都看一眼它还在不在。
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        match api.version().await {
            Ok(version) => {
                logs.finish_startup();
                publish(|state| {
                    state.phase = Phase::Running;
                    state.version = version.version.clone();
                    state.logs = logs.tail();
                });
                if !version.meta {
                    // 原版 Clash 没有 `/rules` 与 TUN 那几个端点，摆出来也是
                    // 残的。说清楚，别让用户对着一个拨不动的开关猜。
                    report(t!("clash.runtime.not_meta"));
                }
                break;
            }
            Err(error) => {
                if let Ok(Some(status)) = child.try_wait() {
                    return Some(exited_early(status.to_string(), &logs));
                }
                if tokio::time::Instant::now() >= deadline {
                    return Some(t!(
                        "clash.runtime.start_timeout",
                        secs => START_TIMEOUT.as_secs(),
                        error => format!("{error:#}"),
                    )
                    .to_string());
                }
            }
        }

        publish(|state| state.logs = logs.tail());
        // 等下一轮，或者等用户/子进程先说话。
        tokio::select! {
            _ = tokio::time::sleep(READY_INTERVAL) => {}
            status = child.wait() => {
                let status = status.map(|status| status.to_string()).unwrap_or_default();
                return Some(exited_early(status, &logs));
            }
            action = input_rx.recv() => {
                match action {
                    // 还没起来就要停：走人，收尾归上面。
                    Some(Action::Stop) => {
                        stopping();
                        return None;
                    }
                    // 还没起来时的其它指令一律忽略：没有实例可说话。
                    Some(_) => {}
                    None => {
                        stopping();
                        return None;
                    }
                }
            }
        }
    }

    // 起来了：先取一份规则（它不会自己变，只在起的时候与手动刷新时取），
    // 再开始按拍子问节点表。
    let mut rules = api.rules().await.ok();
    if let Ok(nodes) = api.proxies().await {
        let mode = api.configs().await.ok().as_ref().and_then(mode_of);
        session.publish_live(&nodes, rules.as_ref(), mode);
        if let Ok(connections) = api.connections().await {
            session.publish_connections(&connections);
        }
    }

    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            action = input_rx.recv() => match action {
                None | Some(Action::Stop) => {
                    stopping();
                    return None;
                }
                Some(action) => {
                    if let Some(reason) = act(action, &mut session).await {
                        return Some(reason);
                    }
                }
            },
            status = child.wait() => {
                let status = status.map(|status| status.to_string()).unwrap_or_default();
                return Some(exited_early(status, &logs));
            }
            _ = ticker.tick() => {
                // 取不到就**留着上一份**：一次本机 HTTP 打个喷嚏不该把界面
                // 上的节点表擦成空白。
                if let Ok(nodes) = api.proxies().await {
                    if rules.is_none() {
                        rules = api.rules().await.ok();
                    }
                    // 模式也跟着采：重载配置、别的客户端改了它，界面都该跟上。
                    let mode = api.configs().await.ok().as_ref().and_then(mode_of);
                    session.publish_live(&nodes, rules.as_ref(), mode);
                }
                if let Ok(connections) = api.connections().await {
                    session.publish_connections(&connections);
                }
            }
        }
    }
}

/// 从 `GET /configs` 的回数里读出分流模式。
fn mode_of(configs: &Value) -> Option<String> {
    configs
        .get("mode")
        .and_then(Value::as_str)
        .map(|mode| mode.to_lowercase())
}

/// 办一道指令。`Some(原因)` 是"这一下把这个实例弄没了"。
async fn act(action: Action, session: &mut Session) -> Option<String> {
    match action {
        // 上面已经拦掉了，这里只是把 `match` 补全。
        Action::Stop => None,
        Action::Refresh => match session.api.proxies().await {
            Ok(nodes) => {
                let rules = session.api.rules().await.ok();
                session.publish_live(&nodes, rules.as_ref(), None);
                None
            }
            Err(error) => {
                report(t!("clash.runtime.refresh_failed", error => format!("{error:#}")));
                None
            }
        },
        Action::DelayTestDone => {
            match session.api.proxies().await {
                Ok(nodes) => session.publish_live(&nodes, None, None),
                Err(error) => {
                    report(t!("clash.runtime.refresh_failed", error => format!("{error:#}")))
                }
            }
            // 数据先出、计数后落：界面看到计数归零时，新数字已经在了。
            publish(|state| state.delay_tests = state.delay_tests.saturating_sub(1));
            None
        }
        Action::TestDelayAll => {
            let groups = session
                .outline
                .as_ref()
                .map(|outline| outline.groups.iter().map(|group| group.name.clone()).collect::<Vec<_>>())
                .or_else(|| {
                    state().live.as_ref().map(|live| {
                        live.groups.iter().map(|group| group.name.clone()).collect::<Vec<_>>()
                    })
                })
                .unwrap_or_default();
            publish(|state| state.delay_tests += 1);
            let api = session.api.clone();
            let input = session.input.clone();
            tokio::spawn(async move {
                for group in groups {
                    if let Err(error) = api.group_delay(&group, TEST_URL, TEST_TIMEOUT).await {
                        log::info!("策略组 {group} 测速没通：{error:#}");
                    }
                }
                let _ = input.send(Action::DelayTestDone);
            });
            None
        }
        Action::CloseConnection(id) => {
            match session.api.close_connection(&id).await {
                Ok(()) => {
                    if let Ok(connections) = session.api.connections().await {
                        session.publish_connections(&connections);
                    }
                }
                Err(error) => report(t!("clash.runtime.connection_failed", error => format!("{error:#}"))),
            }
            None
        }
        Action::CloseConnections => {
            match session.api.close_connections().await {
                Ok(()) => publish(|state| state.connections.clear()),
                Err(error) => report(t!("clash.runtime.connection_failed", error => format!("{error:#}"))),
            }
            None
        }
        Action::ClearLogs => {
            session.clear_logs();
            None
        }
        Action::SetMode(mode) => {
            match session
                .api
                .patch_configs(serde_json::json!({ "mode": mode.as_ref() }))
                .await
            {
                Ok(()) => publish(|state| state.mode = mode),
                Err(error) => {
                    report(t!("clash.runtime.mode_failed", error => format!("{error:#}")))
                }
            }
            None
        }
        Action::SetTun(on) => {
            match session
                .api
                .patch_configs(serde_json::json!({ "tun": { "enable": on } }))
                .await
            {
                Ok(()) => publish(|state| state.tun = on),
                // TUN 要管理员权限（内核要建一块网卡），失败时内核那句话就是
                // 原因，原样摆出来。
                Err(error) => report(t!("clash.runtime.tun_failed", error => format!("{error:#}"))),
            }
            None
        }
        Action::SetSystemProxy(on) => {
            let outcome = match on {
                true => super::system_proxy::enable(session.ports.mixed_port),
                false => super::system_proxy::disable(),
            };
            match outcome {
                Ok(()) => publish(|state| state.system_proxy = on),
                Err(error) => {
                    report(t!("clash.runtime.system_proxy_failed", error => format!("{error:#}")))
                }
            }
            None
        }
        Action::Select { group, node } => {
            match session.api.select(&group, &node).await {
                // 切完**当场**重采一次：等下一个拍子（两秒）才动，
                // 用户会以为没点上。
                Ok(()) => match session.api.proxies().await {
                    Ok(nodes) => session.publish_live(&nodes, None, None),
                    Err(error) => {
                        report(t!("clash.runtime.refresh_failed", error => format!("{error:#}")))
                    }
                },
                Err(error) => {
                    report(t!("clash.runtime.select_failed", error => format!("{error:#}")))
                }
            }
            None
        }
        Action::TestDelay(group) => {
            // 一个组里的节点要一个一个去连，要等几秒（见 [`TEST_TIMEOUT`]）。
            // 放进独立任务：占着会话循环的话，等的这几秒里切模式、选节点都得排队。
            // 测完回送一条 `Refresh` 让界面重采。
            publish(|state| state.delay_tests += 1);
            let api = session.api.clone();
            let input = session.input.clone();
            tokio::spawn(async move {
                if let Err(error) = api.group_delay(&group, TEST_URL, TEST_TIMEOUT).await {
                    report(t!("clash.runtime.delay_failed", error => format!("{error:#}")));
                }
                let _ = input.send(Action::DelayTestDone);
            });
            None
        }
        Action::TestNodeDelay(node) => {
            // 单测一个节点：比整组快得多（内核只连那一个），同样放进独立任务。
            publish(|state| state.delay_tests += 1);
            let api = session.api.clone();
            let input = session.input.clone();
            tokio::spawn(async move {
                // 这里**不弹通知**：一个节点不通是常事（机场的死节点、跨境线路
                // 临时抽风），弹一次通知就是一次打扰。界面上那个数字会变成
                // "超时"，那才是用户要的信息。
                if let Err(error) = api.node_delay(&node, TEST_URL, TEST_TIMEOUT).await {
                    log::info!("节点 {node} 测速没通：{error:#}");
                }
                // 通不通结果都要重采一次：内核已经把这一笔记进历史了。
                let _ = input.send(Action::DelayTestDone);
            });
            None
        }
        Action::Reload(config_yaml) => {
            let path = session.dir.join("config.yaml");
            let previous = std::fs::read_to_string(&path).unwrap_or_default();
            let previous_outline = session.outline.take();
            let text = match config::runtime_config(&config_yaml, &session.ports) {
                Ok(text) => text,
                Err(error) => {
                    report(t!("clash.runtime.bad_config", error => format!("{error:#}")));
                    return None;
                }
            };
            if let Err(error) = std::fs::write(&path, &text) {
                report(t!("clash.runtime.write_config_failed", error => error.to_string()));
                return None;
            }
            // 顺序也跟着新正文走：节点与策略组可能整套换了。
            session.outline = config::outline(&config_yaml).ok();

            let path = path.to_string_lossy().to_string();
            match session.api.reload(&path).await {
                Ok(()) => {
                    // 重读之后规则可能整套换了，立刻重采一份。
                    let rules = session.api.rules().await.ok();
                    match session.api.proxies().await {
                        Ok(nodes) => session.publish_live(&nodes, rules.as_ref(), None),
                        Err(error) => {
                            report(t!("clash.runtime.refresh_failed", error => format!("{error:#}")))
                        }
                    }
                }
                Err(error) => {
                    // 内核没认这份新配置：把盘上那份换回去，免得下一次启动
                    // 读到的是一份它读不动的配置；顺序也退回上一份。
                    let _ = std::fs::write(&path, previous);
                    session.outline = previous_outline;
                    report(t!("clash.runtime.reload_failed", error => format!("{error:#}")));
                }
            }
            None
        }
    }
}

/// 子进程提前没了：把退出码与启动日志的尾巴拼成一句话。
fn exited_early(status: String, logs: &Logs) -> String {
    let tail = logs.reason();
    match tail.is_empty() {
        true => t!("clash.runtime.exited", status => status).to_string(),
        false => t!(
            "clash.runtime.exited_with_logs",
            status => status,
            logs => tail,
        )
        .to_string(),
    }
}

/// 子进程的日志。
///
/// 两段分开收：`startup` 是起来之前那一段（排"为什么起不来"看的就是它），
/// `tail` 是最近的若干行（起来之后看"刚才怎么了"）。
#[derive(Clone)]
struct Logs(Arc<Mutex<LogsInner>>);

struct LogsInner {
    startup: Vec<SharedString>,
    tail: VecDeque<SharedString>,
    warming: bool,
}

impl Logs {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(LogsInner {
            startup: Vec::new(),
            tail: VecDeque::new(),
            warming: true,
        })))
    }

    fn push(&self, line: String) {
        if line.trim().is_empty() {
            return;
        }
        let line = SharedString::from(line.trim_end().to_owned());
        let mut inner = self.0.lock().unwrap_or_else(poisoned);
        if inner.warming && inner.startup.len() < STARTUP_LIMIT {
            inner.startup.push(line.clone());
        }
        if inner.tail.len() == LOG_LIMIT {
            inner.tail.pop_front();
        }
        inner.tail.push_back(line);
    }

    /// 起来了：启动那一段到此为止。
    fn finish_startup(&self) {
        self.0.lock().unwrap_or_else(poisoned).warming = false;
    }

    fn tail(&self) -> Vec<SharedString> {
        self.0
            .lock()
            .unwrap_or_else(poisoned)
            .tail
            .iter()
            .cloned()
            .collect()
    }

    fn clear(&self) {
        let mut inner = self.0.lock().unwrap_or_else(poisoned);
        inner.startup.clear();
        inner.tail.clear();
    }

    /// 报错时那几行。启动那一段有的话用它 —— 原因在那儿。
    fn reason(&self) -> String {
        let inner = self.0.lock().unwrap_or_else(poisoned);
        let source: Vec<&SharedString> = match inner.startup.is_empty() {
            true => inner.tail.iter().collect(),
            false => inner.startup.iter().collect(),
        };
        let start = source.len().saturating_sub(REASON_LINES);
        source[start..]
            .iter()
            .map(|line| line.as_ref())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// 把子进程的一路输出按行收进日志。
///
/// 读不动的那一段（不是 UTF-8 的字节）**不撂挑子**：`lines()` 解码失败时那几个
/// 字节已经从管道里取走了，接着读下一行就是。为一段乱码把管子放下不读，
/// 对面写满管道缓冲区就会卡住 —— 那是"mihomo 莫名其妙不动了"，最难查的一种。
/// 连着几行都读不出来才收手，免得在一遍遍同样的错上打转。
async fn read_lines(stream: impl tokio::io::AsyncRead + Unpin, logs: Logs) {
    use tokio::io::AsyncBufReadExt as _;

    const GIVE_UP_AFTER: usize = 5;

    let mut lines = tokio::io::BufReader::new(stream).lines();
    let mut failures = 0usize;
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                failures = 0;
                logs.push(line);
            }
            Ok(None) => break,
            Err(error) => {
                failures += 1;
                logs.push(format!("（mihomo 的日志里有一段读不出来：{error}）"));
                if failures >= GIVE_UP_AFTER {
                    log::warn!("mihomo 的日志连着 {failures} 段读不出来，不再读它：{error}");
                    break;
                }
            }
        }
    }
}

/// 挑一个当下没人用的本机端口。
///
/// 绑一次 `:0` 拿内核分配的那个号，再把它放掉 —— 标准做法，也是必有一点的
/// 撞车窗口：拿到号到 mihomo 真的绑上去之间，别人可能抢走。真撞上了的后果是
/// 一句"启动失败"，而失败原因是内核日志里那句 `address already in use`，
/// 界面上看得到。
fn free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
    listener.local_addr().ok().map(|address| address.port())
}

/// 每个实例现生一个控制口密钥。本机别的程序也连得上那个口，
/// 没密钥等于把用户的节点列表摆给任何人看。
fn random_secret() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 从外面把一个进程收掉。见 [`shutdown`]。
fn kill_pid(pid: u32) {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = std::process::Command::new("taskkill");
        command.args(["/PID", &pid.to_string(), "/T", "/F"]);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = std::process::Command::new("kill");
        command.args(["-9", &pid.to_string()]);
        command
    };

    match command.status() {
        Ok(status) if status.success() => log::info!("已收掉 mihomo（pid {pid}）"),
        Ok(status) => log::warn!("收 mihomo（pid {pid}）失败：{status}"),
        Err(error) => log::warn!("收 mihomo（pid {pid}）失败：{error}"),
    }
}

/// 目录名里只留安心的字符。订阅 id 是我们自己生成的（slug + 时间戳），
/// 但它是从用户敲的名字来的 —— 名字里万一有个斜杠，这个目录就跑到别处去了。
fn safe_name(id: &str) -> String {
    let name: String = id
        .chars()
        .map(|letter| {
            if letter.is_ascii_alphanumeric() || letter == '-' || letter == '_' || letter == '.' {
                letter
            } else {
                '_'
            }
        })
        .collect();
    match name.trim_matches('.').is_empty() {
        true => "subscription".to_owned(),
        false => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 目录名不能带出路径。钉的是那条不变量本身（一个不含分隔符的普通名字），
    /// 而不是某个特定的替换结果 —— 换个替换字符不该让这条测试红。
    #[test]
    fn an_instance_directory_never_leaves_the_clash_dir() {
        assert_eq!(safe_name("jichang-a-1700000000"), "jichang-a-1700000000");
        for id in [
            "../../windows/system32",
            "..\\..\\windows",
            "/etc/passwd",
            "机场A",
            "a b c",
            "...",
            "",
            ".",
        ] {
            let name = safe_name(id);
            assert!(!name.is_empty(), "{id:?} 该有个兜底名字");
            assert!(!name.contains('/'), "{id:?} -> {name:?} 带出了路径");
            assert!(!name.contains('\\'), "{id:?} -> {name:?} 带出了路径");
            // `.` 与 `..` 是目录本身与上一层，必须被摁掉。
            assert!(name.trim_matches('.') != "", "{id:?} -> {name:?} 是当前或上一层");
        }
        assert_eq!(safe_name("机场A"), "__A");
    }

    /// 密钥每次都不一样，而且是十六进制串。
    #[test]
    fn every_instance_gets_its_own_secret() {
        let first = random_secret();
        let second = random_secret();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|letter| letter.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    /// 挑出来的端口当下真的空着。
    #[test]
    fn a_free_port_is_actually_free() {
        let port = free_port().expect("本机总该有个空口");
        // 能重新绑上去，说明刚才那个是真的放掉了。
        let listener = std::net::TcpListener::bind(("127.0.0.1", port));
        assert!(listener.is_ok(), "端口 {port} 该是空的");
    }

    #[test]
    fn an_idle_state_knows_nothing_is_running() {
        let state = RuntimeState::default();
        assert_eq!(state.phase, Phase::Idle);
        assert!(!state.phase.is_busy());
        assert!(!state.is_live("anything"));
        assert!(state.live.is_none());
    }

    /// 日志分两段收：起来之后内核的话不再冲掉启动那一段。
    #[test]
    fn startup_logs_survive_the_running_noise() {
        let logs = Logs::new();
        logs.push("starting".to_owned());
        logs.push("tun: permission denied".to_owned());
        logs.finish_startup();
        for index in 0..(LOG_LIMIT * 2) {
            logs.push(format!("inbound connection {index}"));
        }
        assert!(logs.reason().contains("permission denied"), "启动那段还在");
        assert_eq!(logs.tail().len(), LOG_LIMIT, "尾巴只留最近这些行");

        // 一个连空行都不算的输入不该占位置。
        let fresh = Logs::new();
        fresh.push("   ".to_owned());
        assert!(fresh.tail().is_empty());
    }

    /// 字节每秒怎么说得短又准。画在顶栏上的是它，那个数字不能长出整格来。
    #[test]
    fn bytes_per_sec_stays_in_four_places() {
        assert_eq!(format_bytes_per_sec(0), "0 B/s");
        assert_eq!(format_bytes_per_sec(88), "88 B/s");
        assert_eq!(format_bytes_per_sec(1023), "1023 B/s");
        // 跨过一档（1024B）。
        assert_eq!(format_bytes_per_sec(1024), "1.0 KB/s");
        assert_eq!(format_bytes_per_sec(1536), "1.5 KB/s");
        assert_eq!(format_bytes_per_sec(88_064), "86.0 KB/s");
        assert_eq!(format_bytes_per_sec(1_048_576), "1.0 MB/s");
        assert_eq!(format_bytes_per_sec(1_610_612), "1.5 MB/s");
        assert_eq!(format_bytes_per_sec(1_073_741_824), "1.0 GB/s");
        assert_eq!(format_bytes_per_sec(1_099_511_627_776), "1.0 TB/s");
    }
}
