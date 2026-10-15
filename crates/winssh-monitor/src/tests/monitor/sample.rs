    use super::*;

    /// 一份完整采集的样子。刻意混了几种真实世界的排版：`df` 带表头、
    /// `/proc/net/dev` 带回环与 docker 网桥、`ps` 的列有对齐空格。
    const FULL: &str = concat!(
        "##hz\n100 4\n",
        "##window\n1000.00 1000.30\n",
        "##uptime\n1000.00 3900.00\n",
        "##load\n0.82 0.94 1.15 2/455 12345\n",
        "##cpu\n",
        "cpu  100 0 50 800 20 0 5 0 0 0\n",
        "cpu0 50 0 25 400 10 0 2 0 0 0\n",
        "cpu1 50 0 25 400 10 0 3 0 0 0\n",
        "@@\n",
        "cpu  130 0 60 830 22 0 5 0 0 0\n",
        "cpu0 65 0 30 415 11 0 2 0 0 0\n",
        "cpu1 65 0 30 415 11 0 3 0 0 0\n",
        "##mem\n",
        "MemTotal:       32791104 kB\n",
        "MemFree:         1000000 kB\n",
        "MemAvailable:   20000000 kB\n",
        "Buffers:          100000 kB\n",
        "Cached:          8000000 kB\n",
        "SwapTotal:       2097148 kB\n",
        "SwapFree:        2097148 kB\n",
        "##net\n",
        "Inter-|   Receive                                                |  Transmit\n",
        " face |bytes    packets errs drop fifo frame compressed multicast|bytes\n",
        "    lo:    1000      10    0    0    0     0          0         0     1000      10    0    0    0     0       0          0\n",
        "  eth0:  200000     500    0    0    0     0          0         0   400000     600    0    0    0     0       0          0\n",
        "docker0:   5000      50    0    0    0     0          0         0     7000      70    0    0    0     0       0          0\n",
        "##disk\n",
        "Filesystem     1024-blocks      Used Available Capacity Mounted on\n",
        "/dev/sda1        527371264 149487616 351000000      30% /\n",
        "##proc\n",
        "1420\t12.8\tS\t4404\tkubelet\n",
        "1980\t8.5\tS\t7000\tetcd\n",
        "##ps\n",
        "   1420 root     /usr/bin/kubelet --config=/var/lib/kubelet/config.yaml\n",
        "   1980 root     etcd --advertise-client-urls=https://10.0.0.1:2379\n",
        "##proccount\n234\n",
        "##end\n",
    );

    #[test]
    fn parses_a_full_probe() {
        let sample = parse_probe(FULL).expect("完整采集应能解析");

        assert_eq!(sample.clock_ticks, 100.);
        assert!((sample.window_secs - 0.3).abs() < 1e-6);
        assert!((sample.uptime_secs - 1000.).abs() < 1e-6);
        assert_eq!(sample.load, [0.82, 0.94, 1.15]);
        assert_eq!(sample.cpu_cores, 2);
        // 进程总数来自 `##proccount`，和被截断后回传的进程列表长度是两回事。
        assert_eq!(sample.process_total, Some(234));
        assert_eq!(sample.processes.len(), 2);

        // 窗口内：总计 +72 节拍，空闲 +32，忙 40/72。
        let busy = CpuTimes::busy_percent(sample.cpu, sample.cpu_after.expect("窗口末尾的采样"))
            .expect("应算得出占用");
        assert!((busy - 55.555_557).abs() < 0.01, "实际 {busy}");
    }

    #[test]
    fn memory_prefers_mem_available() {
        let sample = parse_probe(FULL).unwrap();
        assert_eq!(sample.mem.total_kb, 32_791_104);
        assert_eq!(sample.mem.used_kb(), 12_791_104);
    }

    #[test]
    fn memory_without_mem_available_falls_back_to_subtraction() {
        let mem =
            parse_meminfo("MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\nCached: 200 kB");
        assert_eq!(mem.used_kb(), 650);
    }

    #[test]
    fn network_skips_loopback_and_bridges() {
        let sample = parse_probe(FULL).unwrap();
        assert_eq!(sample.net.rx_bytes, 200_000, "只该算物理网卡");
        assert_eq!(sample.net.tx_bytes, 400_000);
    }

    /// WSL 与一些虚拟化环境里回环叫 `loopback0`，计数器能到几百 MB。
    #[test]
    fn network_skips_loopback_with_a_suffix() {
        let totals = parse_net_dev(
            "loopback0: 699213381 2852127 0 0 0 0 0 6 707629312 2681582 0 0 0 0 0 0\n\
             eth1: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0",
        );
        assert_eq!((totals.rx_bytes, totals.tx_bytes), (100, 200));
    }

    #[test]
    fn disk_reads_the_row_below_the_header() {
        let disk = parse_probe(FULL).unwrap().disk.expect("应有根分区");
        assert_eq!(disk.total_kb, 527_371_264);
        assert_eq!(disk.used_kb, 149_487_616);
    }

    #[test]
    fn processes_take_command_from_ps_and_memory_from_rss() {
        let sample = parse_probe(FULL).unwrap();
        assert_eq!(sample.processes.len(), 2);

        let first = &sample.processes[0];
        assert_eq!(first.pid, 1420);
        assert_eq!(first.user, "root");
        assert_eq!(first.cpu, 12.8);
        assert_eq!(first.state, "S");
        assert!(
            first.command.starts_with("/usr/bin/kubelet --config="),
            "应用 ps 给的完整命令行，实际 {}",
            first.command
        );
        let expected = 4404. / 32_791_104. * 100.;
        assert!((first.mem - expected).abs() < 1e-6);
    }

    #[test]
    fn processes_fall_back_to_comm_without_ps() {
        let text = FULL.replace(
            "   1420 root     /usr/bin/kubelet --config=/var/lib/kubelet/config.yaml\n",
            "",
        );
        let sample = parse_probe(&text).unwrap();
        let first = &sample.processes[0];
        assert_eq!(
            first.command, "kubelet",
            "没有 ps 那一行就用 /proc 的进程名"
        );
        assert_eq!(first.user, "-", "用户名不猜");
    }

    #[test]
    fn light_probe_has_no_second_cpu_sample() {
        let text = concat!(
            "##hz\n100 4\n",
            "##window\n1000.00 1000.00\n",
            "##uptime\n1000.00 3900.00\n",
            "##load\n0.10 0.20 0.30 1/2 3\n",
            "##cpu\ncpu  100 0 50 800 20 0 5 0 0 0\n@@\n\n",
            "##mem\nMemTotal: 1000 kB\nMemAvailable: 400 kB\n",
            "##net\n",
            "##disk\n",
            "##end\n",
        );
        let sample = parse_probe(text).expect("轻量采集也要能解析");
        assert!(sample.cpu_after.is_none());
        assert_eq!(sample.window_secs, 0.);
        assert!(sample.processes.is_empty());
        assert_eq!(sample.process_total, None, "轻量采集不采进程，也没有总数");
        assert!(sample.disk.is_none());
    }

    #[test]
    fn truncated_output_is_an_error() {
        let text = "##hz\n100 4\n##mem\nMemTotal: 1000 kB\n";
        let error = parse_probe(text).expect_err("没有 ##end 必须报错");
        // 不比那句话本身（它在语言表里，跟着语言变），只认这一条错真的是
        // "没跑完"那一条。
        assert_eq!(format!("{error}"), t!("monitor.probe_incomplete"));
    }

    #[test]
    fn non_linux_target_is_an_error() {
        let text = "##hz\n100 4\n##mem\n##end\n";
        let error = parse_probe(text).expect_err("读不到 meminfo 必须报错");
        assert!(format!("{error}").contains("Linux"));
    }
