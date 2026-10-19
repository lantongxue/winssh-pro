//! 接口响应解析与产物挑选。网络请求本身不测 —— 那是云函数的职责，
//! 客户端只保证"拿到 JSON 之后按形状解析、按平台挑包"这两步是对的。

use super::*;

fn asset(kind: &str) -> ReleaseAsset {
    ReleaseAsset {
        kind: kind.to_owned(),
        name: format!("winssh-pro-x64-0.0.6-{kind}.bin"),
        url: format!("https://example.invalid/{kind}"),
        size: 10,
        sha256: "ab".repeat(32),
        signature_url: format!("https://example.invalid/{kind}.minisig"),
    }
}

#[test]
fn parses_a_complete_response() {
    let json = serde_json::json!({
        "update_available": true,
        "current": "0.0.5",
        "latest": "0.0.6",
        "notes": "## v0.0.6\n\n### 新功能\n- 滚动条",
        "published_at": "2026-10-19T08:30:14Z",
        "release_url": "https://github.com/lantongxue/winssh-pro/releases/tag/v0.0.6",
        "assets": [{
            "kind": "installer",
            "name": "winssh-pro-x64-0.0.6-setup.exe",
            "url": "https://github.com/x",
            "size": 1234,
            "sha256": "abc",
            "signature_url": "https://github.com/x.minisig"
        }]
    });
    let parsed: Response = serde_json::from_value(json).expect("response should parse");
    assert!(parsed.update_available);
    assert_eq!(parsed.latest, "0.0.6");
    assert_eq!(parsed.assets.len(), 1);
    assert_eq!(parsed.assets[0].size, 1234);
}

#[test]
fn missing_optional_fields_do_not_fail_parsing() {
    // 老版本云端不带 sha256 / signature_url；缺了就按空串处理，
    // 下载那一步会因此拒绝自动安装，但解析不能失败。
    let json = serde_json::json!({
        "update_available": false,
        "latest": "",
        "assets": [{"kind": "deb", "name": "x.deb", "url": "https://x", "size": 1}]
    });
    let parsed: Response = serde_json::from_value(json).expect("response should parse");
    assert_eq!(parsed.assets[0].sha256, "");
    assert_eq!(parsed.assets[0].signature_url, "");
}

#[test]
fn picks_the_preferred_asset_kind() {
    // 每个平台的首选档不同（Windows 安装版、macOS archive、Linux deb），
    // 这里按当前平台造一对"次选在前、首选在后"的产物，钉住"按优先级挑"。
    let (preferred, fallback) = match platform() {
        "windows" => ("installer", "portable"),
        "macos" => ("archive", "dmg"),
        _ => ("deb", "rpm"),
    };
    let assets = vec![asset(fallback), asset(preferred)];
    let picked = pick_asset(&assets).expect("an asset should be picked");
    assert_eq!(picked.kind, preferred);
}

#[test]
fn returns_none_when_no_asset_matches() {
    assert!(pick_asset(&[]).is_none());
}
