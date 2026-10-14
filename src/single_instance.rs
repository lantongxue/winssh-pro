//! 应用启动互斥：同一个用户下只允许跑一份 WinSSH Pro。
//!
//! 为什么必须互斥：库只有一座（`db::app()`），备份调度、托盘、mihomo 实例、
//! 系统代理也都是"一台机器一份"的东西。两份进程一起跑，轻则各写各的库、
//! 互相看不见对方的改动，重则两边都去改注册表里的系统代理。所以第二份不该
//! 开窗，而是**把已经在跑的那份唤到前面**，然后自己退出。
//!
//! ## 锁
//!
//! 落在用户数据目录（`db::db_dir()`，与库文件同处）下的 `winssh-pro.lock` 上，
//! 用标准库的 `File::try_lock`（Windows 是 `LockFileEx`，Unix 是 `flock`），
//! 不另引依赖。文件锁随进程走：上一份被强杀了，锁由系统收回，不会留一把
//! "没人拿着却还锁着"的假锁 —— 这是它胜过"存在即上锁"的锁文件的地方。
//!
//! ## 唤醒
//!
//! 主实例在 `127.0.0.1` 上开一个临时端口，把端口号写进 `winssh-pro.port`
//! （不写进锁文件：Windows 上被 `LockFileEx` 锁住的文件，别的进程连读都读不了）。
//! 次实例连上去写一个 [`WAKE`] 字，主实例回一个 [`ACK`] 字后把主窗口显示出来
//! 并置顶（托盘里藏着的也一样）。这条口只认这一个字、只用来唤窗口，
//! 不给它别的能力。
//!
//! 与卸载器无关：`scripts/win-installer.iss` 杀完进程还会拿
//! `--restore-system-proxy` 起一次同一个 exe 收拾系统代理，那条路在 `main`
//! 里排在互斥之前，不会被拦下。

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use futures::StreamExt as _;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use gpui_kit::component::Root;
use gpui_kit::{AnyWindowHandle, App, AsyncApp, WindowHandle};

/// 次实例敲门写的那一个字。认它才唤窗口 —— 别的东西连上来（端口扫描、
/// 上一份留下的端口文件恰好撞上了别的程序）一律丢掉。
const WAKE: u8 = 0xa1;
/// 主实例的回话。次实例收到它才算"真敲到了自己人"，否则接着重试。
const ACK: u8 = 0x1a;

const LOCK_FILE: &str = "winssh-pro.lock";
const PORT_FILE: &str = "winssh-pro.port";

/// 次实例最多等多久。主实例刚起来、端口文件还没落的那一小段要等过去 ——
/// 双击两下本来就离得很近，正是要处理的那种时刻。
const WAKE_DEADLINE: Duration = Duration::from_secs(3);

/// 启动时的结论。
pub enum Startup {
    /// 本进程当家，照常开窗。
    Primary(Instance),
    /// 已经有一份在跑。`woke` 说的是有没有成功把它唤到前面 —— 唤不醒
    /// （它还在启动，或者它的唤醒口没开出来）也照样该退出，只是日志不同。
    AlreadyRunning { woke: bool },
}

/// 主实例拿着的东西：锁，以及"有人来敲门"的信号。
///
/// 锁跟着它的生命周期走，所以得拿到进程结束 —— `main` 把它绑在 `run` 外面。
pub struct Instance {
    _lock: File,
    wakes: Option<UnboundedReceiver<()>>,
}

impl Instance {
    /// 取走唤醒信号的收端。只有一份，第二次取是 `None`。
    pub fn take_wakes(&mut self) -> Option<UnboundedReceiver<()>> {
        self.wakes.take()
    }
}

/// 启动时叫一次：要么本进程当家，要么把在跑的那份唤到前面去。
///
/// `dir` 是用户数据目录。给参数是为了测试能换一处目录，生产只有 `main` 里
/// 传 `db::db_dir()` 那一个调用点。
pub fn acquire(dir: &Path) -> Result<Startup> {
    let Some(lock) = try_lock(&dir.join(LOCK_FILE))? else {
        return Ok(Startup::AlreadyRunning {
            woke: wake_existing(&dir.join(PORT_FILE), WAKE_DEADLINE),
        });
    };

    // 开不了唤醒口也得让应用起来 —— 少的只是"再点一次能把窗口唤出来"，
    // 互斥本身（锁）已经在手里了。
    let wakes = match listen(&dir.join(PORT_FILE)) {
        Ok(wakes) => Some(wakes),
        Err(error) => {
            log::warn!("单实例的唤醒口开不出来，再次启动时唤不回这个窗口：{error:#}");
            None
        }
    };
    Ok(Startup::Primary(Instance { _lock: lock, wakes }))
}

/// 打开（必要时创建）锁文件并试着上锁。`None` 是"锁在别人手里"。
fn try_lock(path: &Path) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("打开锁文件失败：{}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        // 不是"被别人占着"而是锁这件事本身做不了（文件系统不支持之类）：
        // 放行，让用户先用上，而不是卡在启动上谁也开不了。
        Err(std::fs::TryLockError::Error(error)) => {
            log::warn!("锁文件上不了锁，这次启动不做互斥（{}）：{error}", path.display());
            Ok(Some(file))
        }
    }
}

/// 主实例开唤醒口：在回环上要一个临时端口，写进端口文件，起线程等人敲门。
fn listen(port_path: &Path) -> Result<UnboundedReceiver<()>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context("在 127.0.0.1 上开监听失败")?;
    let port = listener.local_addr()?.port();
    std::fs::write(port_path, format!("{port}\n"))
        .with_context(|| format!("写端口文件失败：{}", port_path.display()))?;

    let (tx, rx) = mpsc::unbounded();
    std::thread::Builder::new()
        .name("single-instance".to_owned())
        .spawn(move || serve(listener, tx))
        .context("起单实例监听线程失败")?;
    Ok(rx)
}

/// 监听线程：一个一个地接。连上来的要么是另一个自己（立刻写一个字就走），
/// 要么是不相干的东西 —— 后者靠读超时打发，所以不必每个连接各开一条线程。
fn serve(listener: TcpListener, tx: UnboundedSender<()>) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        if answer_wake(&mut stream) && tx.unbounded_send(()).is_err() {
            // 收端没了：GPUI 那边已经在退出，没有窗口可唤了。
            return;
        }
    }
}

/// 读一个字，是 [`WAKE`] 就回 [`ACK`]。连上就不说话的（扫描器最爱这么干）
/// 等满读超时放掉。
fn answer_wake(stream: &mut TcpStream) -> bool {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut byte = [0u8; 1];
    if stream.read_exact(&mut byte).is_err() || byte[0] != WAKE {
        return false;
    }
    let _ = stream.write_all(&[ACK]);
    true
}

/// 端口文件读成端口号。首尾空白不算事；`0` 与读不成数字的一律当没有。
fn parse_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|port| *port != 0)
}

/// 次实例那一边：去敲主实例的门，直到敲开或者过了 `deadline`。
fn wake_existing(port_path: &Path, deadline: Duration) -> bool {
    let until = Instant::now() + deadline;
    loop {
        let port = std::fs::read_to_string(port_path)
            .ok()
            .as_deref()
            .and_then(parse_port);
        if let Some(port) = port
            && knock(port)
        {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 敲一次门：写 [`WAKE`]，等 [`ACK`]。端口文件是上一份留下的、那个端口
/// 此刻是别的程序在听的话，等不到回话，算没敲开。
fn knock(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut byte = [0u8; 1];
    stream.write_all(&[WAKE]).is_ok() && stream.read_exact(&mut byte).is_ok() && byte[0] == ACK
}

#[cfg(test)]
#[path = "tests/single_instance.rs"]
mod tests;

/// 主实例那一边：把"有人来敲门"接到窗口上。
///
/// `wake` 是 `main` 传进来的 `tray::show_main_window` —— 收 `&mut App` 而不是
/// `&mut Window`：窗口可能正藏着（托盘那一档），拿 `AnyWindowHandle` 到 GPUI
/// 线程上 update 才是对的做法。收端是移进来的：这个任务跟着 GPUI 跑到底。
pub async fn wake_back(
    mut wakes: UnboundedReceiver<()>,
    main_window: WindowHandle<Root>,
    cx: &mut AsyncApp,
    wake: fn(AnyWindowHandle, &mut App),
) {
    while wakes.next().await.is_some() {
        cx.update(|cx| wake(main_window.into(), cx));
    }
}
