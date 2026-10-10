    use super::*;

    #[test]
    fn failure_reason_prefers_stderr() {
        let output = ExecOutput {
            stdout: String::new(),
            stderr: "\n  sh: docker: not found\n".into(),
            exit_status: Some(127),
        };
        assert!(!output.success());
        assert_eq!(output.failure_reason(), "sh: docker: not found");
    }

    #[test]
    fn missing_exit_status_counts_as_success() {
        let output = ExecOutput::default();
        assert!(output.success());
    }