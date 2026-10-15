    use super::*;
    use serde_json::json;

    fn proxies() -> Value {
        json!({
            "proxies": {
                "DIRECT": { "type": "Direct" },
                "香港 01": { "type": "Shadowsocks", "server": "hk.example.com", "port": 443,
                             "history": [{ "delay": 120 }, { "delay": 88 }] },
                "日本 01": { "type": "Trojan", "server": "jp.example.com", "port": 8443,
                             "history": [{ "delay": 0 }] },
                "德国 01": { "type": "Vmess", "server": "de.example.com", "port": 1234 },
                "PROXY": { "type": "Selector", "now": "香港 01", "all": ["日本 01", "香港 01", "DIRECT"] },
                "自动": { "type": "URLTest", "now": "香港 01", "all": ["香港 01", "日本 01"] },
                "GLOBAL": { "type": "Selector", "now": "PROXY", "all": ["PROXY"] }
            }
        })
    }

    fn rules() -> Value {
        json!({
            "rules": [
                { "type": "DomainSuffix", "payload": "example.com", "proxy": "PROXY" },
                { "type": "GeoIP", "payload": "CN", "proxy": "DIRECT" },
                { "type": "Match", "payload": "", "proxy": "自动" }
            ]
        })
    }

    #[test]
    fn groups_are_told_apart_from_nodes_and_built_ins() {
        let live = LiveState::from_api(&proxies(), Some(&rules()));
        let names: Vec<&str> = live.groups.iter().map(|g| g.name.as_ref()).collect();
        assert_eq!(names, ["GLOBAL", "PROXY", "自动"], "字典序，GLOBAL 也是组");
        assert_eq!(live.groups[1].now.as_deref(), Some("香港 01"));
    }

    /// 组内成员只有名字，协议、地址、端口与测速要另外那张表补齐。
    #[test]
    fn members_get_their_kind_and_delay_from_the_node_table() {
        let mut live = LiveState::from_api(&proxies(), None);
        let facts = NodeFacts::from_api(&proxies());
        for group in &mut live.groups {
            group.fill(&facts);
        }

        let proxy = live.group("PROXY").unwrap();
        assert_eq!(proxy.nodes[0].name.as_ref(), "日本 01");
        assert_eq!(proxy.nodes[0].kind.as_ref(), "Trojan");
        assert_eq!(proxy.nodes[0].server.as_ref(), "jp.example.com");
        assert_eq!(proxy.nodes[0].port.as_deref(), Some("8443"));
        assert_eq!(proxy.nodes[0].delay, None);
        assert!(proxy.nodes[0].tested, "测过，只是没通");

        let hk = proxy.nodes.iter().find(|n| n.name == "香港 01").unwrap();
        assert_eq!(hk.delay, Some(88), "取最近一次");
        assert_eq!(hk.kind.as_ref(), "Shadowsocks");
        assert_eq!(hk.server.as_ref(), "hk.example.com");
        assert_eq!(hk.port.as_deref(), Some("443"));

        // 内置的假节点：查得到协议，但没有地址与端口。
        let de = proxy.nodes.iter().find(|n| n.name == "DIRECT").unwrap();
        assert_eq!(de.kind.as_ref(), "Direct");
        assert!(de.server.is_empty());
        assert_eq!(de.port, None);
        assert!(!de.tested, "从来没测过");
    }

    /// 正文里写的顺序说了算：组按 `proxy-groups` 排，成员按各组自己的顺序排。
    #[test]
    fn the_subscription_decides_the_order() {
        let mut live = LiveState::from_api(&proxies(), None);
        let facts = NodeFacts::from_api(&proxies());
        for group in &mut live.groups {
            group.fill(&facts);
        }
        let outline = crate::config::outline(
            "proxy-groups:\n  - name: 自动\n    type: url-test\n    proxies: [日本 01, 香港 01]\n  \
             - name: PROXY\n    type: select\n    proxies: [香港 01, 日本 01, DIRECT]\n",
        )
        .unwrap();

        let live = live.ordered_by(&outline);
        let names: Vec<&str> = live.groups.iter().map(|g| g.name.as_ref()).collect();
        assert_eq!(
            names,
            ["自动", "PROXY", "GLOBAL"],
            "正文里的在前，GLOBAL 沉底"
        );

        let proxy = live.group("PROXY").unwrap();
        let members: Vec<&str> = proxy.nodes.iter().map(|n| n.name.as_ref()).collect();
        assert_eq!(members, ["香港 01", "日本 01", "DIRECT"], "照正文的顺序");
    }

    /// 规则：驼峰译成正文里那种写法，`GEOIP` 这类不拆词。
    #[test]
    fn rule_types_are_spelled_the_way_the_config_spells_them() {
        let live = LiveState::from_api(&proxies(), Some(&rules()));
        assert_eq!(live.rules[0].kind.as_ref(), "DOMAIN-SUFFIX");
        assert_eq!(live.rules[0].payload.as_ref(), "example.com");
        assert_eq!(live.rules[0].target.as_ref(), "PROXY");
        assert_eq!(live.rules[1].kind.as_ref(), "GEOIP");
        assert_eq!(live.rules[2].kind.as_ref(), "MATCH");
        // 没点名的按驼峰拆。
        assert_eq!(rule_type("DstPort"), "DST-PORT");
        assert_eq!(rule_type("SrcIPASN"), "SRC-IP-ASN");
        assert_eq!(rule_type(""), "");
    }

    #[test]
    fn an_empty_response_is_an_empty_snapshot() {
        let live = LiveState::from_api(&json!({}), None);
        assert!(live.groups.is_empty() && live.rules.is_empty());
    }

    #[test]
    fn connection_snapshot_keeps_the_fields_used_by_the_panel() {
        let connection = ConnectionSummary::from_entry(&json!({
            "id": "conn/1",
            "metadata": {
                "host": "github.com",
                "process": "browser",
                "sourceIP": "127.0.0.1"
            },
            "rule": "DOMAIN-SUFFIX",
            "rulePayload": "github.com",
            "chains": ["🚀 节点选择", "香港 01"],
            "upload": 12,
            "download": 345
        }))
        .expect("connection id");
        assert_eq!(connection.host.as_ref(), "github.com");
        assert_eq!(connection.process.as_ref(), "browser");
        assert_eq!(connection.chains.len(), 2);
        assert_eq!(connection.upload, 12);
        assert_eq!(connection.download, 345);
    }
