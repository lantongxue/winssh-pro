use super::*;
use bollard::models::{
    ContainerCpuStats, ContainerCpuUsage, ContainerMemoryStats, ContainerPidsStats,
    ContainerSummaryStateEnum, PortSummary, PortSummaryTypeEnum,
};
use std::collections::HashMap;

#[test]
fn reference_splits_tag_but_not_registry_port() {
    assert_eq!(split_reference("nginx:1.27"), ("nginx".into(), "1.27".into()));
    assert_eq!(split_reference("nginx"), ("nginx".into(), String::new()));
    // registry:5000/img 里的冒号是端口，不是 tag。
    assert_eq!(
        split_reference("registry:5000/img"),
        ("registry:5000/img".into(), String::new())
    );
    assert_eq!(
        split_reference("registry:5000/img:v2"),
        ("registry:5000/img".into(), "v2".into())
    );
}

#[test]
fn digest_reference_stays_whole() {
    let (name, tag) = split_reference("nginx@sha256:abcd");
    assert_eq!(name, "nginx@sha256:abcd");
    assert!(tag.is_empty());
}

#[test]
fn short_id_strips_sha_prefix() {
    assert_eq!(short_id("sha256:abcdef0123456789"), "abcdef012345");
    assert_eq!(short_id("abcdef0123456789"), "abcdef012345");
}

#[test]
fn stats_cpu_is_delta_over_system_times_cpus() {
    let s = ContainerStatsResponse {
        cpu_stats: Some(ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage {
                total_usage: Some(200),
                ..Default::default()
            }),
            system_cpu_usage: Some(1000),
            online_cpus: Some(2),
            ..Default::default()
        }),
        precpu_stats: Some(ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage {
                total_usage: Some(100),
                ..Default::default()
            }),
            system_cpu_usage: Some(500),
            ..Default::default()
        }),
        memory_stats: Some(ContainerMemoryStats {
            usage: Some(1000),
            limit: Some(2000),
            stats: Some(HashMap::from([("inactive_file".to_owned(), 200u64)])),
            ..Default::default()
        }),
        pids_stats: Some(ContainerPidsStats {
            current: Some(12),
            ..Default::default()
        }),
        ..Default::default()
    };
    let stats = compute_stats(&s);
    // (200-100)/(1000-500) * 2 * 100 = 40%
    assert!((stats.cpu_percent - 40.0).abs() < 1e-9);
    // 内存扣掉 inactive_file：1000 - 200。
    assert_eq!(stats.mem_used, 800);
    assert_eq!(stats.mem_limit, 2000);
    assert_eq!(stats.pids, 12);
}

#[test]
fn summary_conversion_strips_slash_and_shortens_id() {
    let m = ApiContainerSummary {
        id: Some("sha256:abcdef0123456789".into()),
        names: Some(vec!["/web-01".into()]),
        image: Some("nginx:1.27".into()),
        state: Some(ContainerSummaryStateEnum::RUNNING),
        status: Some("Up 3 hours".into()),
        created: Some(1700000000),
        ports: Some(vec![PortSummary {
            ip: Some("0.0.0.0".into()),
            private_port: 80,
            public_port: Some(8080),
            typ: Some(PortSummaryTypeEnum::TCP),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let summary = container_summary(m);
    assert_eq!(summary.name.as_ref(), "web-01");
    assert_eq!(summary.id.as_ref(), "abcdef012345");
    assert_eq!(summary.state, ContainerState::Running);
    assert_eq!(summary.exit_code, None);
    assert_eq!(summary.ports.len(), 1);
    assert_eq!(summary.ports[0].host_port, 8080);
    assert_eq!(summary.ports[0].container_port, 80);
}
