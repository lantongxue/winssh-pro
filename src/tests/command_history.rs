    use super::*;

    #[test]
    fn an_empty_line_is_not_worth_recording() {
        assert!(!should_record(""));
        assert!(!should_record("   \t "));
        assert!(should_record("ls -al"));
    }

    /// 我们替用户敲进去的那一行不是他的命令，不进历史。
    #[test]
    fn our_own_injection_never_lands_in_the_history() {
        assert!(!should_record(crate::terminal::shell_integration::SCRIPT));
    }

    /// 没带退出码不能当成"成功"：那两件事在界面上一个是绿勾、一个是破折号。
    #[test]
    fn an_unknown_exit_code_is_neither_success_nor_failure() {
        let record = |exit_code| CommandRecord {
            id: "cmd-1".into(),
            command: "ls".into(),
            exit_code,
            executed_at: "2026-09-12 10:00:00".into(),
        };
        assert_eq!(record(Some(0)).succeeded(), Some(true));
        assert_eq!(record(Some(1)).succeeded(), Some(false));
        assert_eq!(record(None).succeeded(), None);
    }
