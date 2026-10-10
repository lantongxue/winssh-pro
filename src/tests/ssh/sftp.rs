    use std::time::Duration;

    use super::*;
    use crate::ssh::exec::fake;

    /// 对端不接子系统时，错误里要有那句人话，而不是一个裸的 `Request denied`。
    ///
    /// 假的 sshd（见 [`fake`]）不提供 SFTP 子系统，直接回 `CHANNEL_FAILURE` ——
    /// 真实环境里 `sshd_config` 没配 `Subsystem sftp` 就是这样。
    #[test]
    fn a_server_without_sftp_says_so() {
        crate::db::runtime().block_on(async {
            let (handle, seen) = fake::connect("", "", true).await;

            let result = tokio::time::timeout(Duration::from_secs(10), open_session(&handle)).await;

            let error = match result {
                Ok(Ok(_)) => panic!("没有子系统的连接不该返回会话"),
                Ok(Err(error)) => error,
                Err(_) => panic!("对端已经答过话，不该一直等下去"),
            };

            let message = format!("{error:#}");
            assert!(
                message.contains(t!("ssh_sftp.subsystem_refused").as_ref()),
                "错误应说明是子系统的问题，实际：{message}"
            );
            assert_eq!(seen.subsystem(), "sftp", "请求的子系统名字要送对");
        });
    }