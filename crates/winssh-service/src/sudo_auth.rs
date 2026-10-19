//! SFTP 提权时 sudo 要的密码从哪来，以及按主机设置翻出这一回的 [`SftpLaunch`]。
//!
//! 握手本身在 [`crate::ssh::sftp`]：它只在 sudo 真的开口要密码时才来问
//! [`SudoAuth`]，NOPASSWD 的机器一次也不会问。这里定"问的时候给什么"：
//!
//! 1. 本进程里这台主机上一次被 sudo 收下的那个（[`cache`]，只在内存里，
//!    重启就没了）；
//! 2. 主机的登录密码 —— sudo 要的是**登录账号自己的**密码，密码登录的主机
//!    绝大多数时候就是它；
//! 3. 都没有、或者都被拒了，才经界面塞进来的钩子（[`SudoAsk`]）弹框问人。
//!
//! 一个候选只试一次：sudo 默认只给三次机会，同一个错密码喂两遍是在浪费。
//! 被拒的缓存当场作废。
//!
//! SFTP 面板、终端底部的 SFTP 栏与智能助手的传输都从 [`sftp_launch`] 拿开法，
//! 所以同一台主机上不论谁先输过一次密码，后来的都直接用上。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use gpui_kit::SharedString;

use crate::host::{AuthMethod, Host};
use crate::ssh::{SftpLaunch, SudoAuth};
use winssh_domain::settings::sftp::SftpLaunchMode;

/// 弹框问人时给界面的那几句话。
#[derive(Clone, Debug)]
pub struct SudoPrompt {
    /// 主机显示名。
    pub host: SharedString,
    /// 登录账号 —— sudo 要的是它的密码，不是 `-u` 那个用户的。
    pub user: SharedString,
    /// 上一次给的被拒了。
    pub retry: bool,
}

/// 界面塞进来的"问人"钩子：可以在任意线程上调（握手跑在数据层 runtime 上），
/// 实现负责把请求送回界面线程开对话框。`None` 是用户取消。
pub type SudoAsk =
    Arc<dyn Fn(SudoPrompt) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync>;

/// 按主机设置翻出这一回 SFTP 通道的开法。
///
/// `mode` 给 `Some` 是面板上"以 sudo 重新打开 / 取消提权"那一下的临时覆盖，
/// 只管那一个面板；`None` 照主机表单里存的走（智能助手永远走这条）。
/// `ask` 是 `None` 时，没有缓存也没有登录密码就只能取消。
pub fn sftp_launch(host: &Host, mode: Option<SftpLaunchMode>, ask: Option<SudoAsk>) -> SftpLaunch {
    match mode.unwrap_or(host.sftp.mode) {
        SftpLaunchMode::Subsystem => SftpLaunch::Subsystem,
        // 自定义档的命令是空的（老库手改、或者面板覆盖到这一档）：没什么可发的，
        // 退回标准子系统，总比 exec 一个空串强。
        SftpLaunchMode::Custom if host.sftp.command.trim().is_empty() => SftpLaunch::Subsystem,
        SftpLaunchMode::Custom => SftpLaunch::Custom {
            command: host.sftp.command.trim().to_owned(),
        },
        SftpLaunchMode::Sudo => SftpLaunch::Sudo {
            user: host.sftp.sudo_user().map(str::to_owned),
            auth: Arc::new(HostSudoAuth::new(host, ask)),
        },
    }
}

/// 本进程里各主机被 sudo 收下过的密码，按主机 id 记。只在内存里。
fn cache() -> MutexGuard<'static, HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// 一台主机的 [`SudoAuth`]。面板每开一次通道新建一个；智能助手那一份跨多次传输
/// 共用，所以"试过哪些候选"在密码被收下时清零（见 [`SudoAuth::accepted`]）。
pub struct HostSudoAuth {
    host_id: String,
    prompt: SudoPrompt,
    login_password: Option<String>,
    ask: Option<SudoAsk>,
    /// 这一回已经喂过的候选，以及最近喂出去的是不是缓存里那个。
    state: Mutex<Tried>,
}

#[derive(Default)]
struct Tried {
    fed: Vec<String>,
    last_from_cache: bool,
}

impl HostSudoAuth {
    pub fn new(host: &Host, ask: Option<SudoAsk>) -> Self {
        let login_password = (host.auth_method == AuthMethod::Password
            && !host.password.is_empty())
        .then(|| host.password.to_string());
        Self {
            host_id: host.id.to_string(),
            prompt: SudoPrompt {
                host: host.name.clone(),
                user: host.user.clone(),
                retry: false,
            },
            login_password,
            ask,
            state: Mutex::new(Tried::default()),
        }
    }

    /// 下一个还没试过的现成候选：先缓存、再登录密码。`retry` 时上一个若来自
    /// 缓存，先把缓存作废 —— 它已经被 sudo 拒了。
    fn next_known(&self, retry: bool) -> Option<String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if retry && state.last_from_cache {
            cache().remove(&self.host_id);
        }
        let cached = cache().get(&self.host_id).cloned();
        let pick = [(cached, true), (self.login_password.clone(), false)]
            .into_iter()
            .find_map(|(candidate, from_cache)| {
                candidate
                    .filter(|password| !state.fed.contains(password))
                    .map(|password| (password, from_cache))
            });
        let (password, from_cache) = pick?;
        state.fed.push(password.clone());
        state.last_from_cache = from_cache;
        Some(password)
    }
}

impl SudoAuth for HostSudoAuth {
    fn password(&self, retry: bool) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> {
        if let Some(known) = self.next_known(retry) {
            return Box::pin(async move { Some(known) });
        }
        let Some(ask) = self.ask.clone() else {
            return Box::pin(async { None });
        };
        let prompt = SudoPrompt {
            // 现成的候选都试过了还在问，就是给错了 —— 哪怕这是第一次弹框。
            retry: retry || !self.state.lock().map(|s| s.fed.is_empty()).unwrap_or(true),
            ..self.prompt.clone()
        };
        if let Ok(mut state) = self.state.lock() {
            state.last_from_cache = false;
        }
        ask(prompt)
    }

    fn accepted(&self, password: &str) {
        cache().insert(self.host_id.clone(), password.to_owned());
        // 这一回结束了，"试过哪些"清零：智能助手那一份开法是跨多次传输共用的，
        // 不清的话下一条通道会把刚被收下的这个当成"试过了"跳过，转头去问人。
        if let Ok(mut state) = self.state.lock() {
            *state = Tried::default();
        }
    }
}

#[cfg(test)]
#[path = "tests/service/sudo_auth.rs"]
mod tests;
