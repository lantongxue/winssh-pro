    use super::*;

    #[test]
    fn light_script_has_no_sampling_window() {
        let script = script(&ProbeOptions::light());
        assert!(!script.contains("sleep"), "轻量采集不该有采样窗口");
        assert!(!script.contains("##proc"), "轻量采集不该带进程列表");
        assert!(script.contains("##load"), "状态栏要的负载必须在");
        assert!(script.trim_end().ends_with("echo '##end'"));
    }

    #[test]
    fn full_script_samples_twice_and_limits_processes() {
        let script = script(&ProbeOptions::full());
        assert!(script.contains("p1=$(cat /proc/[0-9]*/stat"));
        assert!(script.contains("p2=$(cat /proc/[0-9]*/stat"));
        assert!(script.contains("sleep 0.3"));
        assert!(script.contains(&format!("head -n {TOP_PROCESSES}")));
        // 总数在截断之前用全量 `$top` 数出来，单独一段回传。
        assert!(
            script.contains("echo '##proccount'"),
            "要回传真正的进程总数"
        );
        assert!(
            script.find("grep -c .").unwrap() < script.find("head -n").unwrap(),
            "总数得在 head 截断之前算"
        );
    }

    #[test]
    fn all_processes_script_sorts_but_does_not_truncate() {
        let script = script(&ProbeOptions::all_processes());
        assert!(
            script.contains("p1=$(cat /proc/[0-9]*/stat"),
            "仍要采样窗口"
        );
        assert!(script.contains("sort -k2,2nr"), "仍要按 CPU 排序");
        assert!(!script.contains("head -n"), "全部进程不截断");
    }

    /// 脚本整体是单引号里嵌 awk 程序拼出来的，awk 里再出现单引号就会把外层
    /// 的引号提前闭合。这条断言把那种改动挡在编译测试里。
    #[test]
    fn awk_program_has_no_single_quote_inside() {
        let inner = PROC_AWK
            .strip_prefix(
                "awk -v hz=\"$hz\" -v pgkb=\"$pgkb\" -v u1=\"$u1\" -v u2=\"${u2:-$u1}\" '",
            )
            .expect("PROC_AWK 的开头变了");
        let body = inner.strip_suffix('\'').expect("PROC_AWK 的结尾变了");
        assert!(!body.contains('\''), "awk 程序体里不能有单引号");
    }
