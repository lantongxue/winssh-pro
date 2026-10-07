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

    // TUN 那一块：正文里写了就照它的，缺哪一项补哪一项。
    //
    // 补的是"网卡建起来能用"所必需的那几样，而不是替用户拿主意：运行中拨
    // TUN 开关走的是一句 `PATCH /configs`（见 [`crate::clash::runtime`]），
    // 内核照的就是这份参数 —— 少了 `auto-route`，网卡是建起来了，可没有任何
    // 流量会走进去，界面上开关亮着、网却是不通的，那种坏法最难查。
    //
    // `enable` 出厂写成关：起一个实例不等于要接管整台机器的路由，那一步得
    // 用户自己拨。
    let mut tun = match root.get("tun") {
        Some(Value::Mapping(mapping)) => mapping.clone(),
        _ => Mapping::new(),
    };
    fill(&mut tun, "enable", Value::from(false));
    fill(&mut tun, "stack", Value::from("mixed"));
    fill(&mut tun, "auto-route", Value::from(true));
    fill(&mut tun, "auto-detect-interface", Value::from(true));
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
        Value::from("https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/geoip.metadb"),
    );
    fill(
        &mut geox,
        "geosite",
        Value::from("https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/geosite.dat"),
    );
    fill(
        &mut geox,
        "mmdb",
        Value::from("https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/country.mmdb"),
    );
    fill(
        &mut geox,
        "asn",
        Value::from("https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/GeoLite2-ASN.mmdb"),
    );
    root.insert(Value::from("geox-url"), Value::Mapping(geox));
    fill(&mut root, "geo-auto-update", Value::from(false));

    serde_yaml_ng::to_string(&Value::Mapping(root))
        .context("生成 mihomo 运行配置失败")
        .map_err(Into::into)
}

/// 映射表里没有这个键才写进去 —— 用户正文里写过的值一律不动。
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
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
mixed-port: 7890
allow-lan: true
proxies:
  - name: 香港 01
    type: ss
    server: hk1.example.com
    port: 443
    cipher: aes-256-gcm
    password: secret
  - name: 日本 01
    type: trojan
    server: jp1.example.com
    port: 8443
    password: secret
proxy-groups:
  - name: PROXY
    type: select
    proxies: [香港 01, 日本 01]
  - name: 自动
    type: url-test
    use: [机场A]
rules:
  - DOMAIN-SUFFIX,example.com,PROXY
  - GEOIP,CN,DIRECT,no-resolve
  - AND,((DOMAIN-SUFFIX,a.com),(NETWORK,tcp)),PROXY
  - MATCH,自动
"#;

    #[test]
    fn the_outline_reads_nodes_groups_and_rules() {
        let outline = outline(SAMPLE).expect("样例是一份能读的配置");
        assert_eq!(outline.nodes.len(), 2);
        assert_eq!(outline.nodes[0].name.as_ref(), "香港 01");
        assert_eq!(outline.nodes[0].kind.as_ref(), "ss");
        assert_eq!(outline.nodes[0].server.as_ref(), "hk1.example.com");
        assert_eq!(outline.nodes[0].port.as_deref(), Some("443"));
        assert_eq!(outline.nodes[0].address(), "hk1.example.com:443");
        assert_eq!(outline.nodes[1].address(), "jp1.example.com:8443");
        assert_eq!(outline.mixed_port, Some(7890));

        assert_eq!(outline.groups.len(), 2);
        assert_eq!(outline.groups[0].kind.as_ref(), "select");
        assert_eq!(outline.groups[0].members.len(), 2);
        // `use` 引用的 provider 名也在成员里 —— 静态这一眼只有它的名字。
        assert_eq!(outline.groups[1].members[0].as_ref(), "机场A");
    }

    /// 规则里带逗号的那几种（逻辑规则、`no-resolve`）不能拆散。
    #[test]
    fn rules_split_on_top_level_commas_only() {
        let outline = outline(SAMPLE).unwrap();
        assert_eq!(outline.rules[0].kind.as_ref(), "DOMAIN-SUFFIX");
        assert_eq!(outline.rules[0].payload.as_ref(), "example.com");
        assert_eq!(outline.rules[0].target.as_ref(), "PROXY");

        assert_eq!(outline.rules[1].payload.as_ref(), "CN");
        assert_eq!(outline.rules[1].target.as_ref(), "DIRECT");

        let logical = &outline.rules[2];
        assert_eq!(logical.kind.as_ref(), "AND");
        assert_eq!(logical.payload.as_ref(), "((DOMAIN-SUFFIX,a.com),(NETWORK,tcp))");
        assert_eq!(logical.target.as_ref(), "PROXY");

        // 没有内容的规则：内容空着，走谁照旧。
        assert_eq!(outline.rules[3].kind.as_ref(), "MATCH");
        assert_eq!(outline.rules[3].payload.as_ref(), "");
        assert_eq!(outline.rules[3].target.as_ref(), "自动");

        // 没有逗号的（写坏的）：整条就是类型，另两格空着。
        // 走全路径：上面那个 `outline` 局部变量把同名函数遮住了。
        let broken = crate::clash::config::outline("rules:\n  - MATCH\n").unwrap();
        assert_eq!(broken.rules[0].kind.as_ref(), "MATCH");
        assert_eq!(broken.rules[0].target.as_ref(), "");
    }

    /// 交给 mihomo 的那份配置：端口与外控归我们，别的照原文。
    #[test]
    fn the_runtime_config_overrides_only_what_we_must_own() {
        let ports = RuntimePorts {
            mixed_port: 54321,
            api_port: 54322,
            secret: "s3cret".to_owned(),
        };
        let generated = runtime_config(SAMPLE, &ports).unwrap();
        let root = parse_root(&generated).unwrap();

        assert_eq!(number_at(&root, "mixed-port"), Some(54321));
        assert_eq!(
            text_at(&root, "external-controller").as_deref(),
            Some("127.0.0.1:54322")
        );
        assert_eq!(text_at(&root, "secret").as_deref(), Some("s3cret"));
        // 订阅里写着 `allow-lan: true`，我们不听它的。
        assert_eq!(root.get("allow-lan"), Some(&Value::from(false)));

        // 原文里的东西一条不少。
        let outline = outline(&generated).unwrap();
        assert_eq!(outline.nodes.len(), 2);
        assert_eq!(outline.groups.len(), 2);
        assert_eq!(outline.rules.len(), 4);
    }

    /// 单独写着 `port` / `socks-port` 的订阅：那几个口删掉，只留混合口。
    #[test]
    fn the_runtime_config_drops_the_other_listeners() {
        let yaml = "port: 7890\nsocks-port: 7891\nredir-port: 7892\nproxies: []\n";
        let ports = RuntimePorts {
            mixed_port: 1080,
            api_port: 1081,
            secret: "x".to_owned(),
        };
        let root = parse_root(&runtime_config(yaml, &ports).unwrap()).unwrap();
        assert!(root.get("port").is_none());
        assert!(root.get("socks-port").is_none());
        assert!(root.get("redir-port").is_none());
        assert_eq!(number_at(&root, "mixed-port"), Some(1080));
    }

    /// TUN 那一块：没写就补一份能用的，写了就一个键都不动。
    #[test]
    fn the_tun_block_is_filled_in_but_never_overwritten() {
        let ports = RuntimePorts {
            mixed_port: 1080,
            api_port: 1081,
            secret: "x".to_owned(),
        };

        // 正文里没有 tun：补上，而且是关着的 —— 起实例不等于接管路由。
        let root = parse_root(&runtime_config("proxies: []\n", &ports).unwrap()).unwrap();
        let tun = match root.get("tun") {
            Some(Value::Mapping(tun)) => tun.clone(),
            other => panic!("该补出 tun 那一块：{other:?}"),
        };
        assert_eq!(tun.get("enable"), Some(&Value::from(false)));
        assert_eq!(tun.get("stack"), Some(&Value::from("mixed")));
        assert_eq!(tun.get("auto-route"), Some(&Value::from(true)));
        assert_eq!(tun.get("auto-detect-interface"), Some(&Value::from(true)));
        assert!(tun.get("dns-hijack").is_some());

        // 正文里写了：照它的，缺的才补。
        let yaml = "tun:\n  enable: true\n  stack: system\n  device: utun9\n";
        let root = parse_root(&runtime_config(yaml, &ports).unwrap()).unwrap();
        let tun = match root.get("tun") {
            Some(Value::Mapping(tun)) => tun,
            other => panic!("{other:?}"),
        };
        assert_eq!(tun.get("enable"), Some(&Value::from(true)), "用户说了算");
        assert_eq!(tun.get("stack"), Some(&Value::from("system")));
        assert_eq!(tun.get("device"), Some(&Value::from("utun9")));
        assert_eq!(
            tun.get("auto-route"),
            Some(&Value::from(true)),
            "缺的那几样补上"
        );
    }

    #[test]
    fn things_that_are_not_clash_configs_say_so() {
        // base64 的节点列表：解出来是字符串，不是映射表。
        assert!(outline("c3M6Ly9ZV1Z6TFRJMU5pMW5ZMjA2").is_err());
        // YAML 数组也不是。
        assert!(outline("- a\n- b\n").is_err());
        // 空的更不是。
        assert!(outline("   \n").is_err());
        // 是 YAML、是映射表，但没有节点也没有策略组：读得出来，但不可用。
        let bare = outline("port: 7890\n").unwrap();
        assert!(!bare.is_usable());
        assert!(check("port: 7890\n").is_err());
    }

    /// BOM 是记事本存的，不该让整份配置读不出来。
    #[test]
    fn a_byte_order_mark_does_not_break_the_first_key() {
        let outline = outline("\u{feff}proxies:\n  - name: A\n    type: ss\n    server: s\n").unwrap();
        assert_eq!(outline.nodes.len(), 1);
    }
}