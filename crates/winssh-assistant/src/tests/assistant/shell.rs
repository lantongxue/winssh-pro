
    use super::*;

    /// 按本机 shell 造一条命令（超时给足，别让正常用例被收走）。
    fn command(text: &str, timeout_secs: u64) -> LocalCommand {
        LocalCommand {
            command: text.to_owned(),
            shell: shell_kind(),
            timeout_secs,
        }
    }

    fn idle_cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    /// 参数：命令原文去掉首尾空白；超时夹进 [1, 300]；shell 跟着本机系统定；
    /// 空命令现在就报，不等批准之后才失败。
    #[test]
    fn parse_picks_the_local_shell_and_clamps_the_timeout() {
        let parsed = parse(r#"{"command":"  Get-Process  ","timeout_secs":9999}"#).unwrap();
        assert_eq!(parsed.command, "Get-Process");
        assert_eq!(parsed.shell, shell_kind(), "shell 由本机系统定，不是模型说的");
        assert_eq!(parsed.timeout_secs, MAX_TIMEOUT_SECS);
        assert_eq!(
            parse(r#"{"command":"x","timeout_secs":0}"#).unwrap().timeout_secs,
            1
        );
        assert_eq!(parse(r#"{"command":"   "}"#), Err(COMMAND_EMPTY.to_owned()));
        assert!(parse("{}").is_err(), "缺 command 是不合规矩的参数");
    }

    /// 交给 shell 的那几个参数：PowerShell 不读 profile、非交互、整段是 `-Command`
    /// 后面的一条脚本；bash 就是 `-c`。
    #[test]
    fn each_shell_gets_its_own_argv() {
        let (program, args) = ShellKind::Bash.argv("echo hi");
        assert_eq!(program, "bash");
        assert_eq!(args, vec!["-c".to_owned(), "echo hi".to_owned()]);

        let (program, args) = ShellKind::PowerShell.argv("Get-Process");
        assert_eq!(program, "powershell");
        assert!(args.iter().any(|a| a == "-NoProfile"), "不读用户 profile：{args:?}");
        assert_eq!(args.last().unwrap(), "Get-Process", "命令整段是最后那个参数");
    }

    /// 端到端：跑一条无害命令，退出码 0、stdout 里有那行字、没被终止。
    /// 用的是本机真的 shell（Windows PowerShell / 类 Unix bash），命令按平台选。
    #[test]
    fn a_harmless_command_runs_and_reports_its_exit_code() {
        let text = if cfg!(windows) {
            "Write-Output winssh-shell"
        } else {
            "echo winssh-shell"
        };
        let outcome = winssh_db::runtime()
            .block_on(execute(command(text, 30), idle_cancel()))
            .unwrap();
        assert_eq!(outcome.payload["exit_status"], 0);
        let stdout = outcome.payload["stdout"].as_str().unwrap();
        assert!(stdout.contains("winssh-shell"), "stdout 该有那一行：{stdout:?}");
        assert!(outcome.payload.get("terminated").is_none());
    }

    /// 非零退出码是正常结果，原样报给模型（不当错误抛）。
    #[test]
    fn a_failing_command_keeps_its_nonzero_exit_code() {
        let outcome = winssh_db::runtime()
            .block_on(execute(command("exit 7", 30), idle_cancel()))
            .unwrap();
        assert_eq!(outcome.payload["exit_status"], 7);
    }

    /// 超时到点把进程收走，标上 `terminated`，而不是真等满那条命令的时长。
    #[test]
    fn a_command_outliving_its_timeout_is_terminated() {
        let sleepy = if cfg!(windows) {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        };
        let started = Instant::now();
        let outcome = winssh_db::runtime()
            .block_on(execute(command(sleepy, 1), idle_cancel()))
            .unwrap();
        assert!(outcome.payload.get("terminated").is_some(), "该报被终止");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "别真等满 30 秒： {:?}",
            started.elapsed()
        );
    }
