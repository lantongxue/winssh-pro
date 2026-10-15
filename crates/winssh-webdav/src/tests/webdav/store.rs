// 备份落点对着回环 WebDAV 服务器（挂载在 `/dav`）跑一遍列 / 写 / 读 / 删。
//
// 线上踩过的坑（openlist，端点 `https://alist.test.jindashen.cn/dav`）：迁出
// OpenDAL 之后第一版发请求漏拼了挂载点，`PROPFIND` 打到站点根的
// `/winssh_backups` 上，服务器回 405，"备份与恢复"整页报错。这几条守的就是
// "落点在挂载点之下"。

    use super::*;
    use crate::fake_server::{FakeServer, PASSWORD, USER};

    fn store(server: &FakeServer, root: &str) -> WebdavStore {
        WebdavStore::new(&server.endpoint(), root, USER, PASSWORD).unwrap()
    }

    #[test]
    fn first_backup_creates_the_folder_under_the_mount() {
        let server = FakeServer::start();
        let store = store(&server, "/winssh_backups");
        winssh_db::runtime().block_on(async {
            // 落点还不存在：列出来是空的，不是错误。
            assert!(store.list().await.unwrap().is_empty());
            store
                .write("winssh_backup_20261013_120000.wspbak", b"payload".to_vec())
                .await
                .unwrap();
            assert_eq!(
                store.list().await.unwrap(),
                vec![("winssh_backup_20261013_120000.wspbak".to_string(), 7)]
            );
            assert_eq!(
                store.read("winssh_backup_20261013_120000.wspbak").await.unwrap(),
                b"payload"
            );
        });
        // 落在服务器的 `/dav/winssh_backups/` 下（假服务器的键不含挂载点）——
        // 与 OpenDAL 那会儿的位置逐字一致，老备份原地认得出来。
        assert!(server.is_dir("/winssh_backups"));
        assert_eq!(
            server.file("/winssh_backups/winssh_backup_20261013_120000.wspbak").as_deref(),
            Some(&b"payload"[..])
        );
    }

    #[test]
    fn existing_backups_are_listed_and_deleted() {
        let server = FakeServer::start();
        server.put_dir("/winssh_backups");
        server.put_file("/winssh_backups/winssh_backup_1.wspbak", b"one");
        server.put_dir("/winssh_backups/other");
        let store = store(&server, "/winssh_backups");
        winssh_db::runtime().block_on(async {
            // 目录不算副本，只交文件。
            assert_eq!(
                store.list().await.unwrap(),
                vec![("winssh_backup_1.wspbak".to_string(), 3)]
            );
            store.delete("winssh_backup_1.wspbak").await.unwrap();
            assert!(store.list().await.unwrap().is_empty());
        });
    }

    #[test]
    fn nested_root_is_created_level_by_level() {
        let server = FakeServer::start();
        let store = store(&server, "a/b/c");
        winssh_db::runtime()
            .block_on(store.write("x.wspbak", b"x".to_vec()))
            .unwrap();
        assert!(server.is_dir("/a") && server.is_dir("/a/b") && server.is_dir("/a/b/c"));
        assert!(server.file("/a/b/c/x.wspbak").is_some());
    }

    #[test]
    fn wrong_password_is_an_error_not_an_empty_list() {
        let server = FakeServer::start();
        let store = WebdavStore::new(&server.endpoint(), "/winssh_backups", USER, "wrong").unwrap();
        let error = winssh_db::runtime().block_on(store.list()).unwrap_err();
        assert!(format!("{error:#}").contains("401"));
    }

    #[test]
    fn endpoint_without_the_mount_reports_405() {
        // 端点只写了站点根：站点根不是 WebDAV，回 405。必须报出来、说清楚该怎么改，
        // 不能当成"还没有备份"给一个空列表。
        let server = FakeServer::start();
        let endpoint = format!("http://{}", server.addr);
        let store = WebdavStore::new(&endpoint, "/winssh_backups", USER, PASSWORD).unwrap();
        let error = winssh_db::runtime().block_on(store.list()).unwrap_err();
        assert!(format!("{error:#}").contains("405"));
    }
