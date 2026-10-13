    use super::*;
    use crate::host::{AuthMethod, HostDraft};
    use crate::service::HostService;

    /// 一台建在内存库里的主机。历史挂得上去，得先真有这台主机 ——
    /// 外键是真的，随手编一个 id 插不进去。
    async fn fixture() -> (CommandHistoryService, HostService, String) {
        let hosts = HostService::in_memory().await.expect("建内存库失败");
        let host = hosts
            .insert_host(
                "默认分组",
                HostDraft {
                    vnc: Default::default(),
                    rdp: Default::default(),
                    serial: Default::default(),
                    easytier: Default::default(),
                    s3: Default::default(),
                    ftp: Default::default(),
                    webdav: Default::default(),
                    kind: crate::host::HostKind::Ssh,
                    name: "测试机".into(),
                    user: "root".into(),
                    hostname: "10.0.0.1".into(),
                    port: 22,
                    platform: String::new(),
                    auth_method: AuthMethod::Password,
                    password: String::new(),
                    key_content: String::new(),
                    key_passphrase: String::new(),
                    jump_host_id: None,
                    tags: Vec::new(),
                    proxy: Default::default(),
                },
            )
            .await
            .expect("建主机失败");
        // 同一条连接 —— 各开一条的话，内存库是各自独立的一座，
        // 历史那边看不见刚建的这台主机。
        let history = CommandHistoryService {
            repo: CommandHistoryRepository,
            db: hosts.connection(),
        };
        (history, hosts, host.id.to_string())
    }

    fn run(command: &str, exit_code: Option<i32>) -> CommandRun {
        CommandRun {
            command: command.to_owned(),
            exit_code,
        }
    }

    #[test]
    fn history_comes_back_newest_first() {
        db::runtime().block_on(async {
            let (history, _hosts, host_id) = fixture().await;
            for command in ["first", "second", "third"] {
                history
                    .record(&host_id, run(command, Some(0)))
                    .await
                    .expect("记录失败");
            }
            let rows = history.load_for_host(&host_id).await.expect("读取失败");
            let commands: Vec<_> = rows.iter().map(|r| r.command.as_ref()).collect();
            assert_eq!(commands, vec!["third", "second", "first"]);
        });
    }

    /// 没拿到退出码的那一条要原样存成"不知道"，读回来还是"不知道"。
    #[test]
    fn an_unknown_exit_code_survives_the_round_trip() {
        db::runtime().block_on(async {
            let (history, _hosts, host_id) = fixture().await;
            history
                .record(&host_id, run("systemctl status nginx", None))
                .await
                .expect("记录失败");
            let rows = history.load_for_host(&host_id).await.expect("读取失败");
            assert_eq!(rows[0].exit_code, None);
            assert_eq!(rows[0].succeeded(), None);
        });
    }

    /// 空命令连库都不该进。
    #[test]
    fn an_empty_command_is_not_stored() {
        db::runtime().block_on(async {
            let (history, _hosts, host_id) = fixture().await;
            history
                .record(&host_id, run("   ", Some(0)))
                .await
                .expect("记录失败");
            assert!(
                history
                    .load_for_host(&host_id)
                    .await
                    .expect("读取失败")
                    .is_empty()
            );
        });
    }

    /// 历史跟着主机走：主机没了，它的历史也该没了，不留孤儿行。
    /// 这一条同时验证了外键真的开着 —— `ON DELETE CASCADE` 在
    /// `PRAGMA foreign_keys=OFF` 时只是一句注释。
    #[test]
    fn deleting_a_host_takes_its_history_with_it() {
        db::runtime().block_on(async {
            let (history, hosts, host_id) = fixture().await;
            history
                .record(&host_id, run("uptime", Some(0)))
                .await
                .expect("记录失败");

            hosts.delete_host(&host_id).await.expect("删主机失败");

            assert!(
                history
                    .load_for_host(&host_id)
                    .await
                    .expect("读取失败")
                    .is_empty(),
                "主机删掉之后它的历史还留着"
            );
        });
    }

    /// 删一条只删那一条，旁边的两条留着。
    #[test]
    fn deleting_one_record_leaves_its_neighbours() {
        db::runtime().block_on(async {
            let (history, _hosts, host_id) = fixture().await;
            for command in ["first", "second", "third"] {
                history
                    .record(&host_id, run(command, Some(0)))
                    .await
                    .expect("记录失败");
            }
            let rows = history.load_for_host(&host_id).await.expect("读取失败");
            let target = rows
                .iter()
                .find(|row| row.command == "second")
                .expect("刚记进去的就是它");

            let deleted = history.delete(&target.id).await.expect("删除失败");
            assert_eq!(deleted, 1);

            let rows = history.load_for_host(&host_id).await.expect("读取失败");
            let commands: Vec<_> = rows.iter().map(|row| row.command.as_ref()).collect();
            assert_eq!(commands, vec!["third", "first"]);
        });
    }

    /// 清空只清这一台，别人的历史不受影响。
    #[test]
    fn clearing_one_host_leaves_the_others_alone() {
        db::runtime().block_on(async {
            let (history, _hosts, host_id) = fixture().await;
            history
                .record(&host_id, run("ls", Some(0)))
                .await
                .expect("记录失败");
            let cleared = history.clear_host(&host_id).await.expect("清空失败");
            assert_eq!(cleared, 1);
            assert!(
                history
                    .load_for_host(&host_id)
                    .await
                    .expect("读取失败")
                    .is_empty()
            );
        });
    }