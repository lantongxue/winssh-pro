//! 监控采集服务：右侧监控面板拿数据的唯一入口。
//!
//! 一条会话一个实例。它握着那条 SSH 连接的 `Handle`（与终端共享，见
//! [`crate::ssh`]），每次采集在同一条连接上另开一条通道跑 [`crate::monitor::probe`]
//! 的脚本，把结果交给 [`MonitorCollector`] 攒成 [`MonitorTelemetry`]。
//!
//! 界面因此完全不知道 `russh`、`/proc` 与 shell 的存在，只会两件事：
//! `poll` 要一份读数，`kill_process` / `container_action` 请求一个动作。
//!
//! 累加器（上一次的累计值、CPU 曲线）藏在服务里而不是界面里：速率算得对不对
//! 取决于"上一次是什么"，那是采集的一部分，不是画法的一部分。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use russh::client::Handle;

use crate::{
    monitor::{MonitorCollector, MonitorTelemetry, ProbeOptions, parse_probe, probe},
    ssh::{SshHandler, exec_script},
};

/// 一次采集最多等多久。远端读 `/proc` 是微秒级的事，超过这个数说明对端在挨打
/// （负载爆表、盘 hang），这一次就放弃，下一次轮询再试。
const PROBE_TIMEOUT: Duration = Duration::from_secs(12);

/// 一个动作（`kill`、`docker restart`）最多等多久。`docker` 起停容器本身就慢。
const ACTION_TIMEOUT: Duration = Duration::from_secs(25);

/// 容器动作。只有这三种，命令由这里拼，界面给不了别的字符串进来。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
}

impl ContainerAction {
    fn verb(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

/// 一条会话的监控采集入口。廉价克隆（两个 `Arc`），可以直接搬进轮询任务里。
#[derive(Clone)]
pub struct MonitorService {
    handle: Arc<Handle<SshHandler>>,
    collector: Arc<Mutex<MonitorCollector>>,
}

impl MonitorService {
    pub fn new(handle: Arc<Handle<SshHandler>>) -> Self {
        Self {
            handle,
            collector: Arc::new(Mutex::new(MonitorCollector::new())),
        }
    }

    /// 采一次，产出这一刻的遥测。
    pub async fn poll(&self, options: ProbeOptions) -> Result<MonitorTelemetry> {
        let script = probe::script(&options);
        let handle = self.handle.clone();

        let started = Instant::now();
        let output =
            crate::db::run(async move { exec_script(&handle, &script, PROBE_TIMEOUT).await })
                .await
                .context("在远端执行采集脚本失败")?;
        let elapsed = started.elapsed();

        let sample = parse_probe(&output.stdout).with_context(|| {
            // 解析不了的时候，对端的抱怨比我们的猜测有用（`/bin/sh: not found`、
            // `This service allows sftp connections only.` 之类）。
            match output.stderr.trim().is_empty() {
                true => "解析远端采集结果失败".to_owned(),
                false => format!("远端报错：{}", output.failure_reason()),
            }
        })?;

        // 往返耗时里要扣掉远端故意睡的那段采样窗口，否则"RTT"里混着 300ms 的
        // 自家开销。剩下的仍然含远端读 `/proc` 的时间，所以它是"采集往返"，
        // 比纯网络 RTT 略大。
        let window = Duration::from_secs_f64(sample.window_secs.clamp(0., 5.));
        let latency_ms = elapsed.saturating_sub(window).as_millis().min(u32::MAX as u128) as u32;

        let mut collector = self
            .collector
            .lock()
            .map_err(|_| anyhow::anyhow!("监控累加器已损坏"))?;
        Ok(collector.ingest(sample, latency_ms))
    }

    /// 结束一个远端进程。
    ///
    /// `force` 为真时发 `SIGKILL`（`kill -9`），否则 `SIGTERM` —— 面板上那个
    /// 按钮是显式的破坏性动作，所以命令里写的是哪个信号，按钮的 tooltip 上
    /// 就写的是哪个。
    pub async fn kill_process(&self, pid: u32, force: bool) -> Result<()> {
        let signal = match force {
            true => "-9",
            false => "-15",
        };
        // pid 是 u32，拼不出别的东西来。
        self.run(format!("kill {signal} {pid}")).await
    }

    /// 起停或重启一个容器。
    pub async fn container_action(&self, id: &str, action: ContainerAction) -> Result<()> {
        // 容器 id 是十六进制的，除此之外一个字符都不接 —— 这个字符串是从远端
        // `docker ps` 的输出里来的，直接拼进命令行等于让对端决定我们跑什么。
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("容器 id 不合法：{id}");
        }
        self.run(format!("docker {} {id}", action.verb())).await
    }

    /// 跑一条一次性命令，只关心成没成。
    async fn run(&self, command: String) -> Result<()> {
        let handle = self.handle.clone();
        let output =
            crate::db::run(async move { exec_script(&handle, &command, ACTION_TIMEOUT).await })
                .await?;
        if !output.success() {
            bail!("{}", output.failure_reason());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
            assert!(!telemetry.docker_sampled, "没要 docker 就不该说采过");

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
    fn container_id_must_be_hex() {
        crate::db::runtime().block_on(async {
            let (handle, seen) = crate::ssh::exec::fake::connect("", "", true).await;
            let service = MonitorService::new(handle);

            // 这个字符串是从远端 `docker ps` 的输出里来的，不能直接拼进命令行。
            let error = service
                .container_action("abc; rm -rf /", ContainerAction::Stop)
                .await
                .expect_err("不合法的 id 必须被挡住");
            assert!(format!("{error}").contains("不合法"));
            assert!(seen.script().is_empty(), "一个字节都不该发出去");

            service
                .container_action("c8f921a4aaaa", ContainerAction::Restart)
                .await
                .expect("十六进制 id 应放行");
            assert_eq!(seen.script(), "docker restart c8f921a4aaaa");
        });
    }

    #[test]
    fn kill_sends_the_signal_it_says_it_sends() {
        crate::db::runtime().block_on(async {
            let (handle, seen) = crate::ssh::exec::fake::connect("", "", true).await;
            let service = MonitorService::new(handle);
            service.kill_process(4321, false).await.expect("kill 应成功");
            assert_eq!(seen.script(), "kill -15 4321");
        });
    }

    #[test]
    fn action_verbs_are_fixed() {
        assert_eq!(ContainerAction::Start.verb(), "start");
        assert_eq!(ContainerAction::Stop.verb(), "stop");
        assert_eq!(ContainerAction::Restart.verb(), "restart");
    }
}
