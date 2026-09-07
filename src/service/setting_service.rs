//! 应用级偏好服务：`settings` 表那些"不属于某台主机、某条凭据"的设置。
//!
//! 现在只有一项 —— 全局代理（[`crate::proxy::GlobalProxy`]）。它有两个读者，
//! 分别在两条互不相通的路上：
//!
//! - **设置窗口**在 gpui 线程上读写它，要的是立刻拿得到、改完立刻画出来；
//! - **拨号**发生在 tokio 那一侧（[`crate::ssh::connection`]），手上没有 `App`，
//!   而且每开一条会话就要读一次。
//!
//! 所以库是正本，[`crate::proxy`] 里另留一份进程级缓存：启动时 [`Self::prime`]
//! 灌一次，之后每次 [`Self::save_global_proxy`] 两边一起写。少了缓存的话，
//! 要么让拨号那条路去查库（多一次 await 与一条失败路径），要么让界面
//! 每帧查库 —— 两样都不划算。

use anyhow::Result;
use sea_orm::DatabaseConnection;

use crate::{
    db,
    proxy::{self, GlobalProxy},
    repository::SettingRepository,
};

/// 全局代理在 `settings` 表里的键。
const GLOBAL_PROXY_KEY: &str = "proxy.global";

/// 应用级偏好的业务入口。廉价克隆：界面各处各拿一份，共享同一条连接。
#[derive(Clone)]
pub struct SettingService {
    repo: SettingRepository,
    db: DatabaseConnection,
}

impl SettingService {
    /// 接上应用那一座库（[`db::app`]）。
    pub fn open() -> Self {
        Self {
            repo: SettingRepository,
            db: db::app().conn.clone(),
        }
    }

    /// 内存库：测试用的独立一座。
    #[cfg(test)]
    async fn in_memory() -> Result<Self> {
        Ok(Self {
            repo: SettingRepository,
            db: db::connect(None).await?,
        })
    }

    /// 启动时把全局代理从库里读进进程级缓存。
    ///
    /// 同步阻塞一次：这一步在开窗之前，读的是一行；而它必须在第一条会话
    /// 建起来之前完成 —— 晚一步的话，开机就自动连上的那些主机会以"没有
    /// 全局代理"直连出去，而用户明明配过。
    ///
    /// 读不出来只记一行日志：库坏了不该让应用开不起来，界面照常打得开，
    /// 用户在设置里重新配一次就修好了。
    pub fn prime(&self) {
        let this = self.clone();
        let loaded = db::runtime().block_on(async move { this.load_global_proxy().await });
        match loaded {
            Ok(global) => proxy::set_global(global),
            Err(error) => log::warn!("全局代理读不出来，这次运行按未启用处理：{error:#}"),
        }
    }

    /// 库里那份全局代理。没写过就是出厂档（不使用代理）。
    pub async fn load_global_proxy(&self) -> Result<GlobalProxy> {
        let this = self.clone();
        db::run(async move {
            let raw = this.repo.get(&this.db, GLOBAL_PROXY_KEY).await?;
            Ok(proxy::decode_global(&raw.unwrap_or_default()))
        })
        .await
    }

    /// 写回全局代理，并同时换掉进程里那份缓存。
    ///
    /// 缓存先换：界面改完这一格就该立刻按新的走，而落库是异步的。落库失败
    /// 只影响"下次启动还记不记得"，不该让这一次的改动看起来没生效。
    pub async fn save_global_proxy(&self, global: GlobalProxy) -> Result<()> {
        proxy::set_global(global.clone());
        let this = self.clone();
        let encoded = proxy::encode_global(&global);
        db::run(async move {
            this.repo
                .set(&this.db, GLOBAL_PROXY_KEY, &encoded)
                .await
                .map_err(Into::into)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::ProxyConfig;

    /// 这几条都会动进程级的那份缓存（`save_global_proxy` 一并写它），
    /// 所以和别处动它的测试共用同一把锁。
    use crate::proxy::exclusive_for_test as exclusive;

    fn configured() -> GlobalProxy {
        GlobalProxy {
            enabled: true,
            config: ProxyConfig {
                host: "10.0.0.9".into(),
                port: 1080,
                username: "deploy".into(),
                password: " spaces matter ".into(),
            },
        }
    }

    /// 没写过就是出厂档；写一次读得回来；再写一次是覆盖不是插第二行。
    #[test]
    fn the_global_proxy_round_trips_and_overwrites() {
        let _guard = exclusive();
        db::runtime().block_on(async {
            let service = SettingService::in_memory().await.unwrap();
            assert_eq!(
                service.load_global_proxy().await.unwrap(),
                GlobalProxy::default(),
                "没写过就是不使用代理"
            );

            service.save_global_proxy(configured()).await.unwrap();
            assert_eq!(service.load_global_proxy().await.unwrap(), configured());

            // 关掉之后那份地址还留着：下次再打开不必重填一遍。
            let mut off = configured();
            off.enabled = false;
            service.save_global_proxy(off.clone()).await.unwrap();
            assert_eq!(service.load_global_proxy().await.unwrap(), off);
        });
    }

    /// 存一次同时把进程里那份缓存换掉 —— 拨号读的是缓存那一份。
    #[test]
    fn saving_also_updates_the_process_cache() {
        let _guard = exclusive();
        db::runtime().block_on(async {
            let service = SettingService::in_memory().await.unwrap();
            service.save_global_proxy(configured()).await.unwrap();
            assert_eq!(proxy::global(), configured());
        });
    }
}
