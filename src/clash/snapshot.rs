//! 运行中实例的读数：上游 JSON 到我们自己的类型。
//!
//! 界面只认这一层。mihomo 改了字段名，要跟着动的只有这个文件里的几个
//! `from_*`；[`super::api`] 那一层只管把 JSON 取回来，不管它长什么样。
//!
//! 两处**顺序**上的讲究，都来自同一个事实：mihomo 的内部表是 Go 的 map，
//! 序列化出来每一行的顺序都不一样。
//!
//! * 策略组与组内成员按**正文里写的顺序**摆（[`LiveState::ordered_by`]）——
//!   枚举顺序好看不好看是次要的，重要的是它**稳定**：同一份订阅点两次刷新，
//!   列表不会跳位置。
//! * 没有正文可依时退回字典序，至少也是稳定的。

use gpui_kit::SharedString;
use serde_json::Value;

use super::config::{RuleSummary, SubscriptionOutline};

/// 一份读数：此刻的节点/策略组，以及此刻生效的分流规则。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveState {
    /// 策略组。全局那一档（`GLOBAL`）在最后。
    pub groups: Vec<LiveGroup>,
    pub rules: Vec<RuleSummary>,
}

impl LiveState {
    /// 从 `/proxies` 与 `/rules` 两份响应译出来。
    pub fn from_api(proxies: &Value, rules: Option<&Value>) -> Self {
        let mut groups = Vec::new();
        if let Some(table) = proxies.get("proxies").and_then(Value::as_object) {
            for (name, entry) in table {
                let kind = entry
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if !is_group(&kind) {
                    continue;
                }
                groups.push(LiveGroup::from_entry(name, &kind, entry));
            }
        }
        // Go 的 map 没有顺序，字典序至少是稳定的。有人拿着正文时再按正文摆。
        groups.sort_by(|left, right| left.name.cmp(&right.name));

        let rules = rules
            .and_then(|value| value.get("rules"))
            .and_then(Value::as_array)
            .map(|rules| rules.iter().map(rule_of).collect())
            .unwrap_or_default();

        Self { groups, rules }
    }

    /// 按订阅正文里写的顺序重排：策略组照 `proxy-groups` 那段，组内成员照
    /// 各自的 `proxies` 顺序。正文里没有的（`GLOBAL`、`use:` 拉来的 provider
    /// 成员）排在后面，顺序不变。
    pub fn ordered_by(mut self, outline: &SubscriptionOutline) -> Self {
        for group in &mut self.groups {
            let declared = outline
                .groups
                .iter()
                .find(|declared| declared.name == group.name)
                .map(|declared| declared.members.as_slice())
                .unwrap_or_default();
            group.ordered_by(declared);
        }

        // 正文里没有的组（`GLOBAL` 是固定那一个）一律沉底，组内顺序保持原样。
        let declared: Vec<&SharedString> = outline.groups.iter().map(|g| &g.name).collect();
        self.groups.sort_by_key(|group| {
            declared
                .iter()
                .position(|name| **name == group.name)
                .unwrap_or(usize::MAX)
        });
        self
    }

    /// 这个组。找不到就是 `None`。
    #[cfg(test)]
    pub fn group(&self, name: &str) -> Option<&LiveGroup> {
        self.groups.iter().find(|group| group.name == name)
    }
}

/// 一个策略组此刻的样子。
#[derive(Clone, Debug, PartialEq)]
pub struct LiveGroup {
    pub name: SharedString,
    /// `Selector` / `URLTest` / `Fallback` / `LoadBalance` / `Relay`。
    pub kind: SharedString,
    /// 此刻选中的那个成员。不是选择器（比如 `url-test`）时是内核自己要用的
    /// 那一个，照样有意义。
    pub now: Option<SharedString>,
    pub nodes: Vec<LiveNode>,
}

impl LiveGroup {
    fn from_entry(name: &str, kind: &str, entry: &Value) -> Self {
        let now = entry
            .get("now")
            .and_then(Value::as_str)
            .filter(|now| !now.is_empty())
            .map(SharedString::from);
        let nodes = entry
            .get("all")
            .and_then(Value::as_array)
            .map(|all| {
                all.iter()
                    .filter_map(Value::as_str)
                    .map(|name| LiveNode {
                        name: SharedString::from(name.to_owned()),
                        // 组内成员只有名字，协议与地址要回 `/proxies` 那张表里查。
                        kind: SharedString::default(),
                        server: SharedString::default(),
                        port: None,
                        delay: None,
                        tested: false,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Self {
            name: SharedString::from(name.to_owned()),
            kind: SharedString::from(kind.to_owned()),
            now,
            nodes,
        }
    }

    /// 补齐成员的协议、地址、端口与最近一次测速 —— 那几样在 `/proxies` 的
    /// 另一段里，由 [`LiveState::from_api`] 的调用方一次性交进来。
    pub fn fill(&mut self, facts: &NodeFacts) {
        for node in &mut self.nodes {
            if let Some(fact) = facts.get(&node.name) {
                node.kind = fact.kind.clone();
                node.server = fact.server.clone();
                node.port = fact.port.clone();
                node.delay = fact.delay;
                node.tested = fact.tested;
            }
        }
    }

    /// 组内成员按 `declared` 的顺序摆，不在里面的沉底。
    fn ordered_by(&mut self, declared: &[SharedString]) {
        if declared.is_empty() {
            return;
        }
        self.nodes.sort_by_key(|node| {
            declared
                .iter()
                .position(|name| *name == node.name)
                .unwrap_or(usize::MAX)
        });
    }
}

/// 组内一个成员。
///
/// 名字与协议来自内核，地址与端口也来自内核 —— `/proxies` 那张表里每个节点都
/// 带着 `server` / `port`。内置的那几个假节点（`DIRECT` / `REJECT`）没有地址，
/// 那两格就空着。
#[derive(Clone, Debug, PartialEq)]
pub struct LiveNode {
    pub name: SharedString,
    /// 协议类型（`Shadowsocks` / `Vmess` / `Trojan`…），查不到时是空串。
    pub kind: SharedString,
    /// 服务器地址。内置节点与查不到的成员是空串。
    pub server: SharedString,
    /// 端口。内核给的是数字，非数字的来源一律当没有。
    pub port: Option<SharedString>,
    /// 最近一次测出来的毫秒数，`None` 是测不出来。
    pub delay: Option<u32>,
    /// 测过没有 —— `delay` 是 `None` 时用来分辨"还没测"与"测了不通"。
    pub tested: bool,
}

/// `/connections` 的一条实时连接。它只保留详情页真正展示的字段，
/// mihomo 不同版本增加的其它统计字段不会让界面模型跟着变。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnectionSummary {
    pub id: SharedString,
    pub host: SharedString,
    pub process: SharedString,
    pub rule: SharedString,
    pub rule_payload: SharedString,
    pub chains: Vec<SharedString>,
    pub upload: u64,
    pub download: u64,
}

impl ConnectionSummary {
    pub fn from_entry(entry: &Value) -> Option<Self> {
        let id = entry.get("id").and_then(Value::as_str)?.to_owned();
        let metadata = entry.get("metadata").and_then(Value::as_object);
        let text = |key: &str| {
            metadata
                .and_then(|metadata| metadata.get(key))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or_default()
                .to_owned()
        };
        let host = ["host", "destinationIP", "destinationPort"]
            .iter()
            .map(|key| text(key))
            .find(|value| !value.is_empty())
            .unwrap_or_default();
        let process = ["process", "processPath", "sourceIP"]
            .iter()
            .map(|key| text(key))
            .find(|value| !value.is_empty())
            .unwrap_or_default();
        let chains = entry
            .get("chains")
            .and_then(Value::as_array)
            .map(|chains| {
                chains
                    .iter()
                    .filter_map(Value::as_str)
                    .map(SharedString::from)
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            id: id.into(),
            host: host.into(),
            process: process.into(),
            rule: entry
                .get("rule")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            rule_payload: entry
                .get("rulePayload")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            chains,
            upload: entry.get("upload").and_then(Value::as_u64).unwrap_or(0),
            download: entry.get("download").and_then(Value::as_u64).unwrap_or(0),
        })
    }
}

/// 全部节点的事实。单独抽出来是因为策略组那一段只有名字 —— 成员到底是个什么
/// 协议、服务器在哪儿、上次测速多少，得回 `/proxies` 的顶层那张表里查。
///
/// 用具名结构而不是元组：要补的字段加一个就得动全部读法，三格的时候还能忍，
/// 五格就是一场事故。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeFact {
    pub kind: SharedString,
    pub server: SharedString,
    pub port: Option<SharedString>,
    pub delay: Option<u32>,
    pub tested: bool,
}

#[derive(Clone, Debug, Default)]
pub struct NodeFacts(std::collections::HashMap<SharedString, NodeFact>);

impl NodeFacts {
    pub fn from_api(proxies: &Value) -> Self {
        let mut facts = Self::default();
        let Some(table) = proxies.get("proxies").and_then(Value::as_object) else {
            return facts;
        };
        for (name, entry) in table {
            let kind = entry
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let server = entry
                .get("server")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            // 内核报的是数字。字符串形式的留个口子，别的一律当没有。
            let port = match entry.get("port") {
                Some(Value::Number(number)) => Some(SharedString::from(number.to_string())),
                Some(Value::String(text)) if !text.is_empty() => {
                    Some(SharedString::from(text.clone()))
                }
                _ => None,
            };
            let (delay, tested) = delay_of(entry);
            facts.0.insert(
                SharedString::from(name.clone()),
                NodeFact {
                    kind: SharedString::from(kind),
                    server: SharedString::from(server),
                    port,
                    delay,
                    tested,
                },
            );
        }
        facts
    }

    fn get(&self, name: &SharedString) -> Option<&NodeFact> {
        self.0.get(name)
    }
}

/// 一个节点的测速记录：`history` 的最后一条说了最近一次的结果。
/// `delay: 0` 是"没测通"，和"还没测过"（`history` 为空）不是一回事。
fn delay_of(entry: &Value) -> (Option<u32>, bool) {
    let Some(history) = entry.get("history").and_then(Value::as_array) else {
        return (None, false);
    };
    let Some(last) = history.last() else {
        return (None, false);
    };
    let delay = last.get("delay").and_then(Value::as_u64).unwrap_or(0);
    match u32::try_from(delay) {
        Ok(delay) if delay > 0 => (Some(delay), true),
        _ => (None, true),
    }
}

/// 这个 `type` 是不是一个"组"。其余的是节点，或者是内核自带的几个假节点
/// （`DIRECT` / `REJECT` / `COMPATIBLE`…）—— 它们只在组成员里出现。
fn is_group(kind: &str) -> bool {
    matches!(
        kind,
        "Selector" | "URLTest" | "Fallback" | "LoadBalance" | "Relay"
    )
}

/// `/rules` 里的一条。
fn rule_of(entry: &Value) -> RuleSummary {
    RuleSummary {
        kind: rule_type(
            entry
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .into(),
        payload: entry
            .get("payload")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        target: entry
            .get("proxy")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
    }
}

/// 内核报的规则类型是驼峰（`DomainSuffix`），正文里写的是大写短横
/// （`DOMAIN-SUFFIX`）。界面上这两处摆的是同一列东西，写法也得是同一种。
///
/// 绝大多数靠驼峰拆词就能对上，`GeoIP` / `GeoSite` / `IPCIDR` 这几个人家
/// 本来就不拆，得点名。
fn rule_type(kind: &str) -> String {
    match kind {
        "GeoIP" => return "GEOIP".to_owned(),
        "GeoSite" => return "GEOSITE".to_owned(),
        "IPCIDR" => return "IP-CIDR".to_owned(),
        "IPCIDR6" => return "IP-CIDR6".to_owned(),
        "SrcIPCIDR" => return "SRC-IP-CIDR".to_owned(),
        "SrcIPCIDR6" => return "SRC-IP-CIDR6".to_owned(),
        "IPSUFFIX" => return "IP-SUFFIX".to_owned(),
        "SrcIPASN" => return "SRC-IP-ASN".to_owned(),
        "IPASN" => return "IP-ASN".to_owned(),
        "RuleSet" => return "RULE-SET".to_owned(),
        "Process" => return "PROCESS-NAME".to_owned(),
        "ProcessPath" => return "PROCESS-PATH".to_owned(),
        "" => return String::new(),
        _ => {}
    }

    let mut out = String::with_capacity(kind.len() + 4);
    for (index, letter) in kind.chars().enumerate() {
        if letter.is_ascii_uppercase() && index > 0 {
            out.push('-');
        }
        out.extend(letter.to_uppercase());
    }
    out
}

#[cfg(test)]
#[path = "../tests/clash/snapshot.rs"]
mod tests;
