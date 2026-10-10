//! 订阅正文的解读，以及交给 mihomo 的那份运行配置怎么生成。
//!
//! 一份订阅正文（机场给的那个 yml）在我们这儿有两个人读：
//!
//! * **界面**读它，是为了在没启动代理的时候也能把"里面有什么"摆出来 ——
//!   节点、策略组、分流规则，见 [`SubscriptionOutline`]；
//! * **mihomo** 读它，但它读的那份不是库里存的原件，而是 [`runtime_config`]
//!   按本次启动挑好的端口与密钥盖过一遍的副本。
//!
//! 为什么不让 mihomo 直接读原件：正文里几乎都写着 `mixed-port: 7890` 这类
//! 端口，而那个口在用户机器上未必空着 —— 撞上就是一句"启动失败"，用户还
//! 不知道撞的是谁。所以端口一律由我们挑（见 [`crate::clash::runtime`]），
//! 启动前盖进副本里；库里存的仍旧是**原样一份**，备份同步过去的就是它。
//!
//! 解读是**尽量读**的：读不懂的键原样留着（mihomo 那边可能认识），读得懂的
//! 拿出来给界面。只有一件事必须真的读出来 —— 根是不是一张映射表，不是的话
//! 这份东西不是 Clash 配置，得当场说清楚。

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::SharedString;
use serde_yaml_ng::{Mapping, Value};

/// 一份订阅里能看出来的东西，右侧详情页摆的就是它。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubscriptionOutline {
    /// 正文里定义的节点（`proxies` 那一段）。
    pub nodes: Vec<NodeSummary>,
    /// 策略组（`proxy-groups` 那一段）。选择器就在这里面。
    pub groups: Vec<GroupSummary>,
    /// 分流规则（`rules` 那一段）。
    pub rules: Vec<RuleSummary>,
    /// 正文里写的混合端口，没写是 `None`。只用来展示"原文这么配的"，
    /// 实际启动用的端口不归它管（见模块文档）。
    pub mixed_port: Option<u16>,
}

impl SubscriptionOutline {
    /// 这份正文里有没有能拿出去连的东西。
    ///
    /// 一条节点都没有的策略组是常见的（全走 `use: 某个 provider`），所以
    /// 判断的是"节点 + 策略组"两样有没有至少一样。
    pub fn is_usable(&self) -> bool {
        !self.nodes.is_empty() || !self.groups.is_empty()
    }
}

/// 正文里的一个节点。
///
/// 服务器与端口**分开放**：界面上它们是两列（地址一列、端口一列），拼成一个
/// `服务器:端口` 再切回来是把已经知道的事丢掉。两者都缺才是空串 —— 端口那一列
/// 用 [`Self::port`] 读，别再从 `address` 上切。
#[derive(Clone, Debug, PartialEq)]
pub struct NodeSummary {
    pub name: SharedString,
    /// 协议类型，例如 `ss` / `vmess` / `trojan` / `hysteria2`。
    pub kind: SharedString,
    /// 服务器地址（域名或 IP）。正文里没写是空串。
    pub server: SharedString,
    /// 端口。正文里没写、或者写的不是数字时是 `None`。
    ///
    /// 用字符串存是因为正文里 `port: 443` 与 `port: "443"` 都有人写，而
    /// `port: "auto"` 这种非数字的也有人写 —— 那一档摆出来比丢掉强。
    pub port: Option<SharedString>,
}

impl NodeSummary {
    /// `服务器:端口` 那一串，给"一格里塞下两样"的地方用（提示、测试断言）。
    /// 界面上的两列各读各的，不经过它。
    pub fn address(&self) -> String {
        match (&self.port, self.server.is_empty()) {
            (Some(port), false) => format!("{}:{port}", self.server),
            _ => self.server.to_string(),
        }
    }
}

/// 正文里的一个策略组。
#[derive(Clone, Debug, PartialEq)]
pub struct GroupSummary {
    pub name: SharedString,
    /// 组类型，例如 `select` / `url-test` / `fallback` / `load-balance`。
    pub kind: SharedString,
    /// 组里可选的节点名，顺序照正文。走 `use`（引用 provider）的组，
    /// 这里放的是 provider 名 —— 那些节点要等 provider 拉下来才知道，
    /// 运行中看 live 的 `/proxies` 才准。
    pub members: Vec<SharedString>,
}

/// 正文里的一条分流规则。
#[derive(Clone, Debug, PartialEq)]
pub struct RuleSummary {
    /// 规则类型，例如 `DOMAIN-SUFFIX` / `GEOIP` / `MATCH`。
    pub kind: SharedString,
    /// 匹配内容。`MATCH` 这种没有内容的规则是空串。
    pub payload: SharedString,
    /// 命中之后走谁：某个策略组、某个节点，或 `DIRECT` / `REJECT`。
    pub target: SharedString,
}

/// 启动时由我们挑定的那几个口与凭据。见 [`runtime_config`]。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimePorts {
    /// 混合端口：HTTP 与 SOCKS 共用一个口，系统代理指的就是它。
    pub mixed_port: u16,
    /// 外部控制（RESTful API）端口，只有本机连它。
    pub api_port: u16,
    /// DNS 监听口。订阅正文里那份 `dns.listen` 不能原样留（见 runtime_config
    /// 里那一处注释）：同一份订阅的别的客户端会绑同一个口。
    pub dns_port: u16,
    /// API 的密钥。每个实例现生一个，不写死、也不落库。
    pub secret: String,
}

/// 解读一份订阅正文。
///
/// 唯一会失败的情况是"根不是一张映射表"或者"根本不是 YAML" —— 那时候
/// 这份东西不是 Clash 配置，越早说清楚越好（导入表单的校验就靠它）。
/// 其余一切读不懂的键都原样放过：mihomo 认识的东西比我们多。
pub fn outline(config_yaml: &str) -> Result<SubscriptionOutline> {
    let root = parse_root(config_yaml)?;

    let mut outline = SubscriptionOutline {
        mixed_port: number_at(&root, "mixed-port").and_then(|n| u16::try_from(n).ok()),
        ..SubscriptionOutline::default()
    };

    // 节点。
    for value in sequence_at(&root, "proxies") {
        let Some(node) = value.as_mapping() else {
            continue;
        };
        let name = text_at(node, "name").unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let (server, port) = endpoint(node);
        outline.nodes.push(NodeSummary {
            name: name.into(),
            kind: text_at(node, "type").unwrap_or_default().into(),
            server: server.into(),
            port,
        });
    }

    // 策略组。
    for value in sequence_at(&root, "proxy-groups") {
        let Some(group) = value.as_mapping() else {
            continue;
        };
        let name = text_at(group, "name").unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let mut members: Vec<SharedString> = sequence_at(group, "proxies")
            .iter()
            .filter_map(|value| text(value))
            .map(SharedString::from)
            .collect();
        // `use: [provider-a]`：组里的节点由 provider 提供，正文里只有 provider
        // 的名字。运行中从 API 看得到真实成员，静态这一眼至少有它的名字。
        members.extend(
            sequence_at(group, "use")
                .iter()
                .filter_map(|value| text(value))
                .map(SharedString::from),
        );
        outline.groups.push(GroupSummary {
            name: name.into(),
            kind: text_at(group, "type").unwrap_or_default().into(),
            members,
        });
    }

    // 分流规则。规则是一条一条的字符串，不是映射表。
    for value in sequence_at(&root, "rules") {
        let Some(rule) = text(value) else {
            continue;
        };
        let (kind, payload, target) = split_rule(&rule);
        outline.rules.push(RuleSummary {
            kind: kind.into(),
            payload: payload.into(),
            target: target.into(),
        });
    }

    Ok(outline)
}

/// 把一份订阅正文变成交给 mihomo 的那份配置。
///
/// 盖掉的只有启动必须要我们说了算的几样：端口、外部控制地址与密钥、
/// `allow-lan`。其余一律照原文 —— 分流规则、DNS、嗅探、TUN 参数都是用户的
/// 订阅说了算，我们不改。
///
/// `allow-lan` 强制关：订阅正文是别人写的，本地代理口开给整个局域网这件事
/// 不能由一份订阅文件替用户决定。
pub fn runtime_config(config_yaml: &str, ports: &RuntimePorts) -> Result<String> {
    let mut root = parse_root(config_yaml)?.clone();

    // 端口只留混合端口这一个监听：HTTP 和 SOCKS 走同一个口，系统代理指过去
    // 就是它。其余几个（`port` / `socks-port` / `redir-port` / `tproxy-port`）
    // 删掉 —— 留着就是几个我们没打算开、还可能撞车的监听。
    root.remove("port");
    root.remove("socks-port");
    root.remove("redir-port");
    root.remove("tproxy-port");
    root.insert(
        Value::from("mixed-port"),
        Value::from(u64::from(ports.mixed_port)),
    );

    root.insert(
        Value::from("external-controller"),
        Value::from(format!("127.0.0.1:{}", ports.api_port)),
    );
    root.insert(Value::from("secret"), Value::from(ports.secret.clone()));
    root.insert(Value::from("allow-lan"), Value::from(false));

    // DNS 监听口与上面删掉的几个端口同一个道理：订阅正文里那份 `dns.listen`
    // （机场模板爱写 127.0.0.1:7874）原样留着，就会和跑着同一份订阅的别的
    // 客户端（FlClash 之流）撞在同一个口上 —— 后起的那个 bind 失败、内核
    // 直接退，界面上只看得见"TUN 拨不动/控制口没应答"。监听口由我们现挑。
    let mut dns = match root.get("dns") {
        Some(Value::Mapping(mapping)) => mapping.clone(),
        _ => Mapping::new(),
    };
    dns.insert(
        Value::from("listen"),
        Value::from(format!("127.0.0.1:{}", ports.dns_port)),
    );
    // dns 上游里指向 loopback 的条目是**别的客户端**的私口：这份订阅就被
    // FlClash 塞过 `proxy-server-nameserver: [udp://127.0.0.1:7874]`。它不在时
    // 节点域名一个都解析不出来，表现正是"国内直连好好的、外网全超时"。
    // 剔掉；剔空的键整个删掉，让 mihomo 回落到 nameserver 那一档。
    for key in [
        "nameserver",
        "fallback",
        "default-nameserver",
        "proxy-server-nameserver",
    ] {
        let Some(Value::Sequence(items)) = dns.get(key).cloned() else {
            continue;
        };
        let kept: Vec<Value> = items
            .into_iter()
            .filter(|item| !is_loopback_upstream(item))
            .collect();
        if kept.is_empty() {
            dns.remove(key);
        } else {
            dns.insert(Value::from(key), Value::Sequence(kept));
        }
    }
    root.insert(Value::from("dns"), Value::Mapping(dns));

    // TUN 那一块：用户后来在设置里改的参数（栈、网卡名、MTU、路由/出口自动）
    // **盖过**订阅正文里写的那一份 —— 订阅正文是机场给的，用户的偏好是用户
    // 后来敲的，两者之间用户那次赢。
    //
    // `enable` 出厂写成关：起一个实例不等于要接管整台机器的路由，那一步得
    // 用户自己拨（也不让用户在设置里改 enable —— 那个开关的发球权归
    // `PATCH /configs` 那一下现场操作）。
    let prefs = super::tun_settings::current();
    let mut tun = match root.get("tun") {
        Some(Value::Mapping(mapping)) => mapping.clone(),
        _ => Mapping::new(),
    };
    fill(&mut tun, "enable", Value::from(false));
    // 用户偏好盖订阅：用 insert 不用 fill。
    tun.insert(Value::from("stack"), Value::from(prefs.stack.as_yaml()));
    if !prefs.device.trim().is_empty() {
        tun.insert(Value::from("device"), Value::from(prefs.device.clone()));
    }
    if prefs.mtu > 0 {
        tun.insert(Value::from("mtu"), Value::from(u64::from(prefs.mtu)));
    }
    tun.insert(Value::from("auto-route"), Value::from(prefs.auto_route));
    tun.insert(
        Value::from("auto-detect-interface"),
        Value::from(prefs.auto_detect_interface),
    );
    fill(
        &mut tun,
        "dns-hijack",
        Value::Sequence(vec![Value::from("any:53")]),
    );
    root.insert(Value::from("tun"), Value::Mapping(tun));

    // 配置 geox-url 使用高速镜像并关闭后台静默自动更新。
    // 正文里若未配置 geox-url，mihomo 在缺少 Geo 数据库时会默认打 GitHub Releases 下载，
    // 在国内网络环境下极易发生超时（context deadline exceeded），导致启动超过 30s 失败或内核直接崩溃退出。
    // 使用 jsdelivr / testingcf CDN 镜像可确保毫秒级下载完毕。
    let mut geox = match root.get("geox-url") {
        Some(Value::Mapping(mapping)) => mapping.clone(),
        _ => Mapping::new(),
    };
    fill(
        &mut geox,
        "geoip",
        Value::from(
            "https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/geoip.metadb",
        ),
    );
    fill(
        &mut geox,
        "geosite",
        Value::from(
            "https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/geosite.dat",
        ),
    );
    fill(
        &mut geox,
        "mmdb",
        Value::from(
            "https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/country.mmdb",
        ),
    );
    fill(
        &mut geox,
        "asn",
        Value::from(
            "https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/GeoLite2-ASN.mmdb",
        ),
    );
    root.insert(Value::from("geox-url"), Value::Mapping(geox));
    fill(&mut root, "geo-auto-update", Value::from(false));

    serde_yaml_ng::to_string(&Value::Mapping(root))
        .context("生成 mihomo 运行配置失败")
        .map_err(Into::into)
}

/// 映射表里没有这个键才写进去 —— 用户正文里写过的值一律不动。
/// 一条 dns 上游是不是指向本机（`127.0.0.1` / `::1` / `localhost`，带不带
/// `udp://` 之类的前缀都算）。这种条目是别的客户端给自己留的私口，继承过来
/// 就是替别人问路。
fn is_loopback_upstream(value: &Value) -> bool {
    let Some(text) = value.as_str() else {
        return false;
    };
    text.contains("127.0.0.1") || text.contains("::1") || text.contains("localhost")
}

fn fill(mapping: &mut Mapping, key: &str, value: Value) {
    if mapping.get(key).is_none() {
        mapping.insert(Value::from(key), value);
    }
}

/// 订阅正文里有没有能识别的东西。导入表单用它给一句"这不是 Clash 配置"。
pub fn check(config_yaml: &str) -> Result<(), SharedString> {
    let outline = outline(config_yaml).map_err(|error| SharedString::from(format!("{error:#}")))?;
    if !outline.is_usable() {
        return Err(t!("clash.form.not_a_config").into());
    }
    Ok(())
}

/// 解析出根映射表，顺带把几种"不是配置"的情形说清楚。
fn parse_root(config_yaml: &str) -> Result<Mapping> {
    // BOM 是 Windows 记事本存 UTF-8 的赠品，libyaml 见到它会当成普通字符，
    // 于是第一行的键就变成了 `﻿proxies`，谁都不认识。
    let text = config_yaml.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        bail!(t!("clash.config.empty"));
    }
    let value: Value = serde_yaml_ng::from_str(text).map_err(|error| {
        anyhow!(
            "{}",
            t!("clash.config.not_yaml", error => error.to_string())
        )
    })?;
    match value {
        Value::Mapping(mapping) => Ok(mapping),
        // 一份 base64 的节点列表解出来是字符串，一份 YAML 数组解出来是序列。
        // 都不是 Clash 配置 —— 但光说"不是映射表"用户看不懂，得告诉他去换
        // 哪种链接：机场面板上挑 "Clash" / "mihomo" 那一栏。
        _ => bail!(t!("clash.config.not_a_mapping")),
    }
}

/// 映射表里某个键的字符串值。数字、布尔也当文本读 —— 配置里
/// `port: 443` 与 `port: "443"` 都有人写。
fn text_at(mapping: &Mapping, key: &str) -> Option<String> {
    mapping.get(key).and_then(text)
}

/// 映射表里某个键的数字值。
fn number_at(mapping: &Mapping, key: &str) -> Option<i64> {
    match mapping.get(key)? {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// 映射表里某个键的序列。不是序列就当没有。
fn sequence_at<'a>(mapping: &'a Mapping, key: &str) -> &'a [Value] {
    match mapping.get(key) {
        Some(Value::Sequence(sequence)) => sequence.as_slice(),
        _ => &[],
    }
}

/// 任意值是文本就取文本。
fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// `服务器` 与 `端口` 那两列。地址或端口缺哪样就少哪样。
///
/// 端口那一格不强制成数字：`port: "auto"` 这种也有人写（尤其订阅正文里），
/// 摆出来比丢掉强；真正没有的时候才是 `None`。
fn endpoint(node: &Mapping) -> (String, Option<SharedString>) {
    let server = text_at(node, "server").unwrap_or_default();
    match number_at(node, "port") {
        Some(port) => (server, Some(SharedString::from(port.to_string()))),
        None => (server, text_at(node, "port").map(SharedString::from)),
    }
}

/// 把一条分流规则拆成"类型 / 匹配内容 / 走谁"。
///
/// 不能直接按逗号切：逻辑规则的内容自带逗号
/// （`AND,((DOMAIN-SUFFIX,a.com),(NETWORK,tcp)),PROXY`），切了就散架。
/// 所以只看**顶层**的逗号 —— 括号里的不算 —— 并且末段的 `no-resolve`
/// 是规则的修饰词，不是"走谁"，得让过去。
fn split_rule(rule: &str) -> (String, String, String) {
    let commas = top_level_commas(rule);
    let Some(&first) = commas.first() else {
        // 没有逗号：整条就是类型，例如写坏的 `MATCH`。
        return (rule.trim().to_owned(), String::new(), String::new());
    };

    let kind = rule[..first].trim().to_owned();
    // 只有一个逗号的规则是"没有内容"的那种：`MATCH,自动` 里的那一格是
    // **走谁**，不是内容 —— 一条没有目标的规则本来就写不成一条规则。
    let Some(&last) = commas.last().filter(|_| commas.len() > 1) else {
        return (kind, String::new(), rule[first + 1..].trim().to_owned());
    };

    // 末段是修饰词（`no-resolve` 之类）时，"走谁"是它前面那一段。
    let tail = rule[last + 1..].trim();
    let (payload, target) = match is_rule_modifier(tail) {
        true => {
            let previous = commas[commas.len() - 2];
            (
                rule[first + 1..previous].trim(),
                rule[previous + 1..last].trim(),
            )
        }
        false => (rule[first + 1..last].trim(), tail),
    };

    (kind, payload.to_owned(), target.to_owned())
}

/// 规则的修饰词：跟在"走谁"后面、不是目标的那几样。
fn is_rule_modifier(field: &str) -> bool {
    matches!(field, "no-resolve" | "src" | "dst")
}

/// 顶层的逗号位置：括号里的不算。
fn top_level_commas(rule: &str) -> Vec<usize> {
    let mut commas = Vec::new();
    let mut depth = 0usize;
    for (index, byte) in rule.bytes().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => commas.push(index),
            _ => {}
        }
    }
    commas
}

use rust_i18n::t;

#[cfg(test)]
#[path = "../tests/clash/config.rs"]
mod tests;
