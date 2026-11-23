    use super::*;
    use crate::monitor::probe::SECTION_PREFIX;

    /// 一份够用的采集输出。段落齐不齐由 [`crate::monitor::sample`] 的测试管，
    /// 这里只要它能走通"脚本 → 遥测"这条路。
    const PROBE: &str = concat!(
        "##hz\n100 4\n",
        "##window\n1000.00 1000.30\n",
        "##uptime\n1000.00 3900.00\n",
        "##load\n0.50 0.40 0.30 1/2 3\n",
        "##cpu\ncpu  1000 0 0 800 0 0 0 0 0 0\ncpu0 500 0 0 400 0 0 0 0 0 0\n",
        "@@\ncpu  1100 0 0 850 0 0 0 0 0 0\ncpu0 550 0 0 425 0 0 0 0 0 0\n",
        "##mem\nMemTotal: 2097152 kB\nMemAvailable: 1048576 kB\n",
        "##net\n  eth0: 1024 1 0 0 0 0 0 0 2048 2 0 0 0 0 0 0\n",
        "##disk\n/dev/sda1 1048576 524288 524288 50% /\n",
        "##proc\n42\t9.5\tR\t1024\tstress\n",
        "##ps\n     42 root     stress --cpu 1\n",
        "##end\n",
    );

    /// 整条采集路：拼脚本 → 在（假的）远端跑 → 解析 → 累加成遥测。
    /// 中间那台 sshd 是进程内的真 `russh` 服务端，走的是真的 SSH 通道。
    #[test]
    fn poll_runs_the_probe_and_returns_telemetry() {
        crate::db::runtime().block_on(async {
            let (handle, seen) = crate::ssh::exec::fake::connect(PROBE, "", true).await;
            let service = MonitorService::new(handle);

            let telemetry = service
                .poll(ProbeOptions::full())
                .await
                .expect("采集应成功");

            // 送出去的是那份采集脚本，而不是别的什么。
            let script = seen.script();
            assert!(script.contains(&format!("{SECTION_PREFIX}proc")));
            assert!(script.starts_with("LC_ALL=C"));

            // 窗口内所有档位共 +150 节拍、空闲 +50 → 忙 100/150。
            assert_eq!(telemetry.cpu_percent, 66.7);
            assert_eq!(telemetry.memory_used_gb, 1.);
            assert_eq!(telemetry.memory_total_gb, 2.);
            assert_eq!(telemetry.disk_used_gb, 0.5);
            assert_eq!(telemetry.uptime, "16 mins");
            assert_eq!(telemetry.load_avg, [0.5, 0.4, 0.3]);

            let process = telemetry.processes.first().expect("应有一条进程");
            assert_eq!(process.pid, 42);
            assert_eq!(process.user, "root");
            assert_eq!(process.cpu, 9.5);
            assert_eq!(process.command, "stress --cpu 1");

            // 往返里扣掉了远端那 0.3 秒采样窗口，回环上剩不下什么。
            assert_eq!(telemetry.latency_ms, 0);
        });
    }

    /// 对端不是 Linux（或者 `exec` 被 sshd 换成了别的东西）：报错要带上对端说的话，
    /// 不能只说"解析失败"。
    #[test]
    fn poll_reports_what_the_remote_said() {
        crate::db::runtime().block_on(async {
            let (handle, _) = crate::ssh::exec::fake::connect(
                "",
                "This service allows sftp connections only.\n",
                true,
            )
            .await;
            let service = MonitorService::new(handle);

            let error = service
                .poll(ProbeOptions::light())
                .await
                .expect_err("解析不了必须报错");
            let text = format!("{error:#}");
            assert!(text.contains("sftp connections only"), "实际：{text}");
        });
    }

    #[test]
    fn kill_sends_the_signal_it_says_it_sends() {
        crate::db::runtime().block_on(async {
            let (handle, seen) = crate::ssh::exec::fake::connect("", "", true).await;
            let service = MonitorService::new(handle);
            service
                .kill_process(4321, false)
                .await
                .expect("kill 应成功");
            assert_eq!(seen.script(), "kill -15 4321");
        });
    }

    /// WSL 那一档的整条采集路：拼脚本 → 经 `wsl.exe` 进发行版的 /bin/sh →
    /// 解析 → 累加成遥测。与 SSH 共用同一份脚本与解析，这里验的是"通道换成
    /// wsl.exe 之后整条路仍然通"。环境敏感（本机得真装着 WSL），按项目规矩
    /// 标 `#[ignore]`：`WSL_TEST_DISTRO=Ubuntu-24.04 cargo test -p winssh-service
    /// wsl -- --ignored`。
    #[test]
    #[cfg(windows)]
    #[ignore = "需要本机装有 WSL，并用 WSL_TEST_DISTRO 指定发行版"]
    fn wsl_poll_reads_the_distro() {
        let Ok(distro) = std::env::var("WSL_TEST_DISTRO") else {
            return;
        };
        crate::db::runtime().block_on(async {
            let service = MonitorService::wsl(distro);
            let telemetry = service
                .poll(ProbeOptions::full())
                .await
                .expect("WSL 采集应成功");
            assert!(telemetry.memory_total_gb > 0., "发行版内存总量应读得出来");
            assert!(!telemetry.processes.is_empty(), "发行版里总有进程在跑");
            assert!(telemetry.process_total.is_some(), "进程总数应单独回传");
            assert!(telemetry.disk_total_gb > 0., "根文件系统容量应读得出来");
        });
    }