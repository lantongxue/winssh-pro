//! 端口转发的数据模型。
//!
//! 术语按设计原型统一（原型里那一层是 `SessionPortForwardModal`）：
//!
//! - **转发规则**（[`PortForward`]）是一段保存下来的配置，"本地 15432 接到
//!   对端的 10.0.4.15:5432"说的就是它。
//! - **隧道**是照着规则在活着的会话上真的建起来的那个监听。它有流量、有连接
//!   数、会失败，这些都在 [`ForwardRuntime`] 里，由
//!   [`crate::service::ForwardService`] 提供，搬字节的是 [`crate::ssh::forward`]。
//!
//! 规则跟着**主机**走，不跟着会话走 —— 落在 `hosts.forwards` 那一列里（见
//! [`crate::db`]）。`-L 15432:10.0.4.15:5432` 讲的是"这台机器背后的那个数据库
//! 怎么映到本地"，换一次会话它还是同一件事；所以关掉标签页再开，列表还在，
//! [`PortForward::enabled`] 的那几条会自己起来。
//!
//! 这个文件只描述规则本身与它的口径（怎么读、怎么写、等价的 `ssh` 命令行长
//! 什么样），一个字节都不搬。

use gpui_kit::SharedString;
use serde::{Deserialize, Serialize};

use crate::host::Host;
use rust_i18n::t;

/// 转发的三档，对应 `ssh` 的三个开关。
///
/// 三档的**监听在哪一侧**是不同的，这是它们唯一的本质区别，别的都是它的后果：
///
/// | 档 | 谁在监听 | 目标由谁定 |
/// | --- | --- | --- |
/// | [`Local`](Self::Local)（`-L`） | 本机 | 规则里写死 |
/// | [`Remote`](Self::Remote)（`-R`） | 远端 | 规则里写死（本机一侧） |
/// | [`Dynamic`](Self::Dynamic)（`-D`） | 本机 | 每条连接自己说（SOCKS5） |
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardKind {
    /// `-L`：本机监听一个端口，连上来的流量经这条 SSH 送到对端能看见的某个地址。
    /// 访问远端数据库、远端 Web 面板走这一档。
    #[default]
    Local,
    /// `-R`：请对端监听一个端口，它那边连上来的流量送回本机的某个地址。
    /// 把本机的服务临时暴露给远端网络走这一档。
    Remote,
    /// `-D`：本机监听一个端口并在上面讲 SOCKS5，目标由每条连接自己给。
    /// 整台机器当跳板用走这一档。
    Dynamic,
}

impl ForwardKind {
    /// 全部三档，顺序照原型里那一组按钮：`-L` / `-R` / `-D`。
    pub const ALL: [Self; 3] = [Self::Local, Self::Remote, Self::Dynamic];

    /// 原型里那一组按钮上的文字。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Local => t!("forward.label_local"),
            Self::Remote => t!("forward.label_remote"),
            Self::Dynamic => t!("forward.label_dynamic"),
        }
        .into()
    }

    /// 规则卡片右上角那枚徽章上的字。
    ///
    /// 不进语言表：这三个全大写的标签照的是 ssh 自己的 `LocalForward` /
    /// `RemoteForward` / `DynamicForward`，两种语言下都是同一个词。停在徽章上
    /// 给的那句人话（[`Self::tagline`]）才是翻的那一份。
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Local => "LOCAL FORWARD",
            Self::Remote => "REMOTE FORWARD",
            Self::Dynamic => "DYNAMIC FORWARD",
        }
    }

    /// 拓扑图中间那一段说明字节往哪儿流。
    pub fn pipe(&self) -> SharedString {
        match self {
            Self::Local => t!("forward.pipe_local"),
            Self::Remote => t!("forward.pipe_remote"),
            Self::Dynamic => t!("forward.pipe_dynamic"),
        }
        .into()
    }

    /// 一句话说清这一档在干什么，不提 `-L`/`-R`/`-D`。
    ///
    /// 三档的开关名字只差一个字母，而字节的方向完全相反 —— 记不住那三个字母
    /// 才是常态。所以每一档都配一句大白话：说的是"什么搬到哪儿"，
    /// 而不是"这个参数叫什么"。
    pub fn tagline(&self) -> SharedString {
        match self {
            Self::Local => t!("forward.tagline_local"),
            Self::Remote => t!("forward.tagline_remote"),
            Self::Dynamic => t!("forward.tagline_dynamic"),
        }
        .into()
    }

    /// [`tagline`](Self::tagline) 之后再补的一两句：什么时候该选它。
    pub fn detail(&self) -> SharedString {
        match self {
            Self::Local => t!("forward.detail_local"),
            Self::Remote => t!("forward.detail_remote"),
            Self::Dynamic => t!("forward.detail_dynamic"),
        }
        .into()
    }

    /// 一个具体到端口号的例子。数字和 [`PRESETS`] 对得上，照着填就能跑。
    ///
    /// 和 [`detail`](Self::detail) 分开，是因为界面上它单独一行、单独一种颜色：
    /// 讲道理的那几句可以跳过不读，例子是照抄就能用的那一行。
    pub fn example(&self) -> SharedString {
        match self {
            Self::Local => t!("forward.example_local"),
            Self::Remote => t!("forward.example_remote"),
            Self::Dynamic => t!("forward.example_dynamic"),
        }
        .into()
    }

    /// 监听端口那一格的标签。`-R` 监听在远端，这一格问的不是同一件事。
    pub fn listen_label(&self) -> SharedString {
        match self {
            Self::Remote => t!("forward.listen_label_remote"),
            _ => t!("forward.listen_label_local"),
        }
        .into()
    }

    /// 这一档要不要填目标地址。动态转发的目标由每条连接自己给，填了也没处用。
    pub fn needs_target(&self) -> bool {
        !matches!(self, Self::Dynamic)
    }

    /// 监听是不是在本机这一侧。决定"在浏览器里打开"和"测试端口"能不能做 ——
    /// 远端监听的那个端口在别人机器上，本机连不上它。
    pub fn listens_locally(&self) -> bool {
        !matches!(self, Self::Remote)
    }
}

/// 一条转发规则。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortForward {
    /// 规则 id。只在这台主机的列表里唯一，够用来对齐"哪一条隧道属于哪一条规则"。
    pub id: SharedString,
    /// 规则名，例如 `PostgreSQL 5432 Local Forward`。
    pub name: SharedString,
    #[serde(rename = "type")]
    pub kind: ForwardKind,
    /// 监听地址。`-L` / `-D` 默认 `127.0.0.1`（只有本机连得上）；写
    /// `0.0.0.0` 就是把这个端口开给局域网，那是显式的选择。
    /// `-R` 时它是**远端**要绑的地址。
    pub bind_address: SharedString,
    /// 监听端口。
    ///
    /// 必须是一个具体的端口（[`Self::check`] 挡住 0）：这条规则要存下来、
    /// 下次开会话还照着起，而"随便给一个"意味着每次的端口都不一样 ——
    /// 那样客户端里配的连接地址下次就不对了。
    pub listen_port: u16,
    /// 目标主机。`-L` 时是**对端**解析得到的地址，`-R` 时是**本机**这一侧的地址。
    /// 动态转发不用它。
    pub target_host: SharedString,
    pub target_port: u16,
    /// 这条规则该不该在跑。
    ///
    /// 存的是"意图"而不是"此刻的实况"：连上会话时按它自动起，界面上按一次
    /// Stop 就翻成假并落库。所以下次开这台主机的会话，上次留着在跑的那几条
    /// 还是在跑 —— 转发是一台机器的常备设施，不该每次重新点一遍。
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    /// 一句说明，列表里不显示，用来提醒自己这条是干什么的。
    #[serde(default)]
    pub description: SharedString,
}

fn enabled_default() -> bool {
    true
}

impl PortForward {
    /// 一条新规则。id 按当下的时间戳取，和 [`crate::service::HostService`] 给
    /// 主机分配 id 是同一个路子。
    pub fn new(kind: ForwardKind) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        Self {
            id: SharedString::from(format!("fwd-{nanos}")),
            name: SharedString::default(),
            kind,
            bind_address: SharedString::from(DEFAULT_BIND),
            listen_port: 0,
            target_host: SharedString::from(DEFAULT_BIND),
            target_port: 0,
            enabled: true,
            description: SharedString::default(),
        }
    }

    /// 监听那一端的 `地址:端口`，例如 `127.0.0.1:15432`。
    pub fn listen_endpoint(&self) -> String {
        format!("{}:{}", self.bind_address, self.listen_port)
    }

    /// 目标那一端的 `地址:端口`。动态转发没有固定目标，那是 `None`。
    pub fn target_endpoint(&self) -> Option<String> {
        self.kind
            .needs_target()
            .then(|| format!("{}:{}", self.target_host, self.target_port))
    }

    /// 在浏览器里打开这条转发的地址。
    ///
    /// 只有本机监听、而且不是 SOCKS5 代理的那一档有意义：远端监听的端口在别人
    /// 机器上，而 SOCKS5 不是 HTTP，浏览器打开它只会看到一堆二进制。
    /// 猜 `http` 而不是 `https`：转发出来的端口多半是内网服务，走的是明文，
    /// 而 `https` 猜错的代价是一个证书错误页。
    pub fn browser_url(&self) -> Option<String> {
        if self.kind != ForwardKind::Local {
            return None;
        }
        // 绑在 `0.0.0.0` 上的端口也是本机连得上的，但那个地址不能直接放进 URL。
        let host = match self.bind_address.as_ref() {
            "0.0.0.0" | "" | "*" => "127.0.0.1",
            other => other,
        };
        Some(format!("http://{host}:{}", self.listen_port))
    }

    /// 等价的 `ssh` 命令行。原型里那颗 "Copy CLI" 复制的就是它。
    ///
    /// 用处是把这条规则交给别人（脚本、同事、CI）：这边点出来的东西，那边
    /// 一行命令就能复现。参数顺序按 `ssh(1)`，不按界面上字段的排法 ——
    /// 粘到终端里要能直接跑。
    pub fn ssh_command(&self, host: &Host) -> String {
        let login = format!("{}@{}", host.user, host.hostname);
        let port = format!("-p {}", host.port);
        match self.kind {
            // -L [bind_address:]port:host:hostport
            ForwardKind::Local => format!(
                "ssh -N -L {}:{}:{}:{} {login} {port}",
                self.bind_address, self.listen_port, self.target_host, self.target_port
            ),
            // -R [bind_address:]port:host:hostport —— 冒号左边是远端要绑的，
            // 右边是本机这一侧的目标，方向和 -L 正好相反。
            ForwardKind::Remote => format!(
                "ssh -N -R {}:{}:{}:{} {login} {port}",
                self.bind_address, self.listen_port, self.target_host, self.target_port
            ),
            // -D [bind_address:]port
            ForwardKind::Dynamic => format!(
                "ssh -N -D {}:{} {login} {port}",
                self.bind_address, self.listen_port
            ),
        }
    }

    /// 校验一条规则能不能起。返回的是一句给用户看的话。
    ///
    /// 挡在这儿而不是等对端拒绝：端口 0 之外的错（没名字、目标空着）根本走不到
    /// 网络那一层，而"绑定失败"这种错要等一次往返才知道，能提前说的就提前说。
    pub fn check(&self) -> Result<(), SharedString> {
        if self.name.trim().is_empty() {
            return Err(t!("forward.check_no_name").into());
        }
        if self.listen_port == 0 {
            return Err(t!("forward.check_listen_port").into());
        }
        if self.bind_address.trim().is_empty() {
            return Err(t!("forward.check_bind_address").into());
        }
        if self.kind.needs_target() {
            if self.target_host.trim().is_empty() {
                return Err(t!("forward.check_target_host").into());
            }
            if self.target_port == 0 {
                return Err(t!("forward.check_target_port").into());
            }
        }
        Ok(())
    }
}

/// 监听地址的出厂值：只有本机连得上。
pub const DEFAULT_BIND: &str = "127.0.0.1";

/// 原型里那一排预设。点一下把整条规则填好，剩下改个名字就能存。
pub struct ForwardPreset {
    pub label: &'static str,
    pub kind: ForwardKind,
    pub listen_port: u16,
    pub target_port: u16,
    /// 说明那一句在语言表里的 key，不是文案本身 —— 这一整排是个 `const`，
    /// 而查表要到运行时才做。读文案走 [`Self::description`]。
    description_key: &'static str,
}

impl ForwardPreset {
    /// 预设片的悬停说明。
    pub fn description(&self) -> SharedString {
        t!(self.description_key).into()
    }
}

/// 四个预设，与原型里那一排片一致。
pub const PRESETS: [ForwardPreset; 4] = [
    ForwardPreset {
        label: "MySQL",
        kind: ForwardKind::Local,
        listen_port: 3306,
        target_port: 3306,
        description_key: "forward.preset_mysql",
    },
    ForwardPreset {
        label: "PostgreSQL",
        kind: ForwardKind::Local,
        listen_port: 5432,
        target_port: 5432,
        description_key: "forward.preset_postgresql",
    },
    ForwardPreset {
        label: "Redis",
        kind: ForwardKind::Local,
        listen_port: 6379,
        target_port: 6379,
        description_key: "forward.preset_redis",
    },
    ForwardPreset {
        label: "SOCKS5",
        kind: ForwardKind::Dynamic,
        listen_port: 1080,
        target_port: 1080,
        description_key: "forward.preset_socks5",
    },
];

/// 一条隧道此刻的实况。规则说"应该怎样"，这个说"现在怎样"。
///
/// 没有"失败原因"这一项：起不来的规则根本不会变成隧道，那时候的原因是
/// [`crate::service::ForwardService::start`] 返回的错误，当场弹给用户，
/// 不留在状态里。这里只描述**跑着的**那些。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForwardRuntime {
    /// 监听建起来了、还在。
    pub active: bool,
    /// 两个方向搬过的字节总数。设计稿里 "Traffic: 4.67 MB" 就是它。
    pub bytes: u64,
    /// 这条隧道上开过多少条连接。
    pub connections: u64,
    /// 真正绑到的端口。正常情况下就是规则里那个数；单独记一份是因为
    /// "该连哪儿"这个问题的答案应当来自实际绑定的结果，而不是来自意图。
    pub listen_port: u16,
}

/// 把字节数说成一句人话。设计稿上是 "4.67 MB" 那一档。
pub fn format_traffic(bytes: u64) -> String {
    const KIB: f64 = 1024.;
    const MIB: f64 = KIB * 1024.;
    const GIB: f64 = MIB * 1024.;
    let bytes = bytes as f64;
    if bytes < KIB {
        return format!("{bytes:.0} B");
    }
    if bytes < MIB {
        return format!("{:.1} KB", bytes / KIB);
    }
    if bytes < GIB {
        return format!("{:.2} MB", bytes / MIB);
    }
    format!("{:.2} GB", bytes / GIB)
}

/// 把一台主机的转发规则编码成 `hosts.forwards` 那一列的内容。
///
/// 空列表编成空串而不是 `[]`：老库补出来的那一列默认就是空串，两者读回来
/// 都是"这台主机没有转发规则"，不必为同一件事留两种写法。
pub fn encode(forwards: &[PortForward]) -> String {
    if forwards.is_empty() {
        return String::new();
    }
    // 序列化一串结构体是不会失败的（没有非字符串键、没有 NaN），
    // 真失败了也不能让"保存主机"整个倒掉 —— 那样列表反而会被清空。
    serde_json::to_string(forwards).unwrap_or_default()
}

/// 从 `hosts.forwards` 那一列读回规则。
///
/// **读不动就当没有**，不往上抛错：这一列是跟着主机一起读出来的，为了一列
/// 坏掉的 JSON 让整个主机列表打不开是不成比例的。真读坏了用户看到的是
/// "这台主机没有转发规则"，再存一次就修好了 —— 日志里留一行说明发生过什么。
pub fn decode(raw: &str) -> Vec<PortForward> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str(raw) {
        Ok(forwards) => forwards,
        Err(error) => {
            log::warn!("hosts.forwards 解析失败，这台主机的转发规则按空处理：{error}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{AuthMethod, Host, MonitorIntervals};

    fn rule(kind: ForwardKind) -> PortForward {
        PortForward {
            id: "fwd-1".into(),
            name: "PostgreSQL".into(),
            kind,
            bind_address: "127.0.0.1".into(),
            listen_port: 15432,
            target_host: "10.0.4.15".into(),
            target_port: 5432,
            enabled: true,
            description: "".into(),
        }
    }

    fn host() -> Host {
        Host {
            vnc: Default::default(),
            kind: crate::host::HostKind::Ssh,
            id: "h1".into(),
            name: "Prod-K8s-Master-01".into(),
            user: "root".into(),
            hostname: "192.168.1.100".into(),
            port: 22,
            platform: "Ubuntu".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            shell_integration: false,
        }
    }

    /// 存得住、读得回，而且一模一样。
    #[test]
    fn rules_round_trip_through_the_column() {
        let rules = vec![rule(ForwardKind::Local), rule(ForwardKind::Dynamic)];
        let encoded = encode(&rules);
        assert_eq!(decode(&encoded), rules);
    }

    /// 空列表与空列都是"这台主机没有转发规则"，不留第二种写法。
    #[test]
    fn an_empty_list_is_an_empty_column() {
        assert_eq!(encode(&[]), "");
        assert!(decode("").is_empty());
        assert!(decode("   ").is_empty());
    }

    /// 一列坏掉的 JSON 不能让整个主机列表打不开。
    #[test]
    fn a_broken_column_reads_as_no_rules() {
        assert!(decode("{ 这不是 JSON").is_empty());
        assert!(decode("[{\"id\":\"x\"}]").is_empty(), "字段缺一半也算坏的");
    }

    /// 老库里存的规则没有 `enabled` 这一项，读回来要是"在跑"，
    /// 不能因为加了一个字段就把用户的转发全停掉。
    #[test]
    fn a_rule_written_before_enabled_existed_still_runs() {
        let raw = r#"[{"id":"fwd-1","name":"pg","type":"local","bind_address":"127.0.0.1",
            "listen_port":15432,"target_host":"10.0.4.15","target_port":5432}]"#;
        let rules = decode(raw);
        assert_eq!(rules.len(), 1);
        assert!(rules[0].enabled);
    }

    /// 三档的命令行方向不能搞混：`-L` 与 `-R` 的冒号两侧含义相反。
    #[test]
    fn the_cli_says_what_each_kind_actually_does() {
        let host = host();
        assert_eq!(
            rule(ForwardKind::Local).ssh_command(&host),
            "ssh -N -L 127.0.0.1:15432:10.0.4.15:5432 root@192.168.1.100 -p 22"
        );
        assert_eq!(
            rule(ForwardKind::Remote).ssh_command(&host),
            "ssh -N -R 127.0.0.1:15432:10.0.4.15:5432 root@192.168.1.100 -p 22"
        );
        assert_eq!(
            rule(ForwardKind::Dynamic).ssh_command(&host),
            "ssh -N -D 127.0.0.1:15432 root@192.168.1.100 -p 22",
            "动态转发没有目标，命令行里也不该出现它"
        );
    }

    /// 只有本地转发能在浏览器里打开：远端监听不在本机，SOCKS5 不是 HTTP。
    #[test]
    fn only_a_local_forward_opens_in_a_browser() {
        assert_eq!(
            rule(ForwardKind::Local).browser_url().as_deref(),
            Some("http://127.0.0.1:15432")
        );
        assert!(rule(ForwardKind::Remote).browser_url().is_none());
        assert!(rule(ForwardKind::Dynamic).browser_url().is_none());

        let mut wildcard = rule(ForwardKind::Local);
        wildcard.bind_address = "0.0.0.0".into();
        assert_eq!(
            wildcard.browser_url().as_deref(),
            Some("http://127.0.0.1:15432"),
            "0.0.0.0 不能直接放进 URL"
        );
    }

    #[test]
    fn a_rule_has_to_say_where_it_listens_and_where_it_goes() {
        let mut nameless = rule(ForwardKind::Local);
        nameless.name = "  ".into();
        assert!(nameless.check().is_err());

        let mut portless = rule(ForwardKind::Local);
        portless.listen_port = 0;
        assert!(portless.check().is_err());

        let mut targetless = rule(ForwardKind::Local);
        targetless.target_port = 0;
        assert!(targetless.check().is_err());

        // 同一条规则改成动态转发就合法了：那一档不需要目标。
        targetless.kind = ForwardKind::Dynamic;
        assert!(targetless.check().is_ok());

        assert!(rule(ForwardKind::Local).check().is_ok());
    }

    /// 三档的说明得真的**各说各的**，而且不许拿 `-L`/`-R`/`-D` 当解释 ——
    /// 这几句话存在的理由就是给记不住那三个字母的人看的。
    #[test]
    fn every_kind_explains_itself_in_plain_words() {
        for kind in ForwardKind::ALL {
            for line in [kind.tagline(), kind.detail(), kind.example()] {
                assert!(!line.trim().is_empty(), "{kind:?} 少了一句说明");
            }
            // 三档的说明都是查表来的，漏一条 key 就会退化成 key 本身
            // （`forward.tagline_local` 那样）。带点就当漏了。
            assert!(
                !kind.tagline().starts_with("forward."),
                "{kind:?} 的说明没查到译文，露出了 key"
            );
            assert!(
                !kind.tagline().contains("-L")
                    && !kind.tagline().contains("-R")
                    && !kind.tagline().contains("-D"),
                "{kind:?} 的一句话结论又绕回开关名字上了"
            );
        }
        let taglines: Vec<_> = ForwardKind::ALL.iter().map(ForwardKind::tagline).collect();
        for (at, tagline) in taglines.iter().enumerate() {
            assert!(
                !taglines[at + 1..].contains(tagline),
                "两档的说明一模一样，那就分不出该选哪一档了"
            );
        }
    }

    #[test]
    fn traffic_reads_like_the_design_says() {
        assert_eq!(format_traffic(0), "0 B");
        assert_eq!(format_traffic(512), "512 B");
        assert_eq!(format_traffic(1024), "1.0 KB");
        assert_eq!(format_traffic(4_897_113), "4.67 MB");
        assert_eq!(format_traffic(3 * 1024 * 1024 * 1024), "3.00 GB");
    }
}
