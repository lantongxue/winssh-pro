    use super::*;
    use std::process::Command;

    /// 送进去的必须是**一行**：调用方在末尾补一个回车就当成一条命令跑。
    /// 里头混进换行的话，对端会把它拆成几条，而中间那几条多半不成句。
    #[test]
    fn the_script_is_a_single_line() {
        assert!(!SCRIPT.contains('\n'));
        assert!(!SCRIPT.contains('\r'));
    }

    /// 四个标记一个都不能少 —— 少 `B` 就不知道提示符停在哪一列，少 `C`
    /// 就不知道命令什么时候开跑，两者缺一条命令都刮不出来。
    #[test]
    fn all_four_marks_are_armed() {
        for mark in ["133;A", "133;B", "133;C", "133;D"] {
            assert!(SCRIPT.contains(mark), "{mark} 没装");
        }
    }

    /// 两档 shell 都要认，而且要有门闩。
    #[test]
    fn both_shells_are_covered_and_guarded() {
        assert!(SCRIPT.contains("BASH_VERSION"));
        assert!(SCRIPT.contains("ZSH_VERSION"));
    }

    /// [`GUARD`] 是另写的一份字面量（`concat!` 不吃常量），两处对不上的话
    /// 门闩失灵不说，命令历史还会把注入那一行当成用户敲的记下来。
    #[test]
    fn the_guard_name_matches_the_script() {
        assert!(SCRIPT.contains(GUARD));
    }

    /// 本机上有哪个 shell 就拿哪个做语法检查（`-n` 只解析不执行）。
    ///
    /// 装不到就跳过：这一行是送去**远端**跑的，本机有没有 bash 不影响功能，
    /// 为它把整条测试判红只会让 Windows 上的 CI 常年是红的。
    fn syntax_check(shell: &str) {
        let Ok(output) = Command::new(shell).arg("-n").arg("-c").arg(SCRIPT).output() else {
            eprintln!("本机没有 {shell}，跳过这一档语法检查");
            return;
        };
        assert!(
            output.status.success(),
            "{shell} 认为这一行不成句：{}",
            String::from_utf8_lossy(&output.stderr),
        );
    }

    /// **两家都要能解析整行**，不只是自己那一档 —— `if/elif` 的每一支在进门
    /// 时都要先过语法，bash 读不懂 zsh 那一段的话，这一行在 bash 上一句都跑
    /// 不了。这正是"一行兼容两家"最容易栽的地方。
    #[test]
    fn bash_parses_the_whole_line() {
        syntax_check("bash");
    }

    #[test]
    fn zsh_parses_the_whole_line() {
        syntax_check("zsh");
    }
