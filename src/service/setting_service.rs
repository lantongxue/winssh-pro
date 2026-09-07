//! 应用级偏好服务：`settings` 表那些"不属于某台主机、某条凭据"的设置。
//!
//! 现在有两项 —— 全局代理（[`crate::proxy::GlobalProxy`]）与终端字体
//! （[`crate::terminal::font::TerminalFont`]）。两者的读法一样刁钻：
//!
//! - **设置窗口**在 gpui 线程上读写它们，要的是立刻拿得到、改完立刻画出来；
//! - **拨号**发生在 tokio 那一侧（[`crate::ssh::connection`]），手上没有 `App`，
//!   而且每开一条会话就要读一次；**终端字体**更极端，每一帧都要读一次。
//!
//! 所以库是正本，[`crate::proxy`] 与 [`crate::terminal::font`] 各留一份进程级
//! 缓存：启动时 [`Self::prime`] 灌一次，之后每次 `save_*` 两边一起写。少了缓存
//! 的话，要么让拨号那条路去查库（多一次 await 与一条失败路径），要么让界面
//! 每帧查库 —— 两样都不划算。

use anyhow::Result;
use sea_orm::DatabaseConnection;

use crate::{
    db,
    proxy::{self, GlobalProxy},
    repository::SettingRepository,
    terminal::font::{self, TerminalFont},
};

/// 全局代理在 `settings` 表里的键。
const GLOBAL_PROXY_KEY: &str = "proxy.global";
/// 终端字体在 `settings` 表里的键。
const TERMINAL_FONT_KEY: &str = "terminal.font";

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

    /// 启动时把这几项从库里读进各自的进程级缓存。
    ///
    /// 同步阻塞一次：这一步在开窗之前，读的是两行；而它必须在第一条会话
    /// 建起来之前完成 —— 晚一步的话，开机就自动连上的那些主机会以"没有
    /// 全局代理"直连出去，而用户明明配过；终端字体晚一步则是第一帧用出厂
    /// 字体画完再跳一下。
    ///
    /// 读不出来只记一行日志：库坏了不该让应用开不起来，界面照常打得开，
    /// 用户在设置里重新配一次就修好了。
    pub fn prime(&self) {
        let this = self.clone();
        let loaded = db::runtime().block_on(async move {
            (
                this.load_global_proxy().await,
                this.load_terminal_font().await,
            )
        });
        match loaded.0 {
            Ok(global) => proxy::set_global(global),
            Err(error) => log::warn!("全局代理读不出来，这次运行按未启用处理：{error:#}"),
        }
        match loaded.1 {
            Ok(terminal_font) => font::set_global(terminal_font),
            Err(error) => log::warn!("终端字体读不出来，这次运行按出厂档处理：{error:#}"),
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

    /// 库里那份终端字体。没写过就是出厂档（跟随主题的等宽字体、13px）。
    pub async fn load_terminal_font(&self) -> Result<TerminalFont> {
        let this = self.clone();
        db::run(async move {
            let raw = this.repo.get(&this.db, TERMINAL_FONT_KEY).await?;
            Ok(font::decode(&raw.unwrap_or_default()))
        })
        .await
    }

    /// 写回终端字体，并同时换掉进程里那份缓存。
    ///
    /// 缓存先换，理由和 [`Self::save_global_proxy`] 一样：改完这一格就该立刻
    /// 按新的画，而落库是异步的。
    pub async fn save_terminal_font(&self, value: TerminalFont) -> Result<()> {
        font::set_global(value.clone());
        let this = self.clone();
        let encoded = font::encode(&value);
        db::run(async move {
            this.repo
                .set(&this.db, TERMINAL_FONT_KEY, &encoded)
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

    /// 终端字体走的是另一行、另一份缓存，读写与出厂档都要各自成立。
    #[test]
    fn the_terminal_font_round_trips_into_its_own_row() {
        let _guard = font::exclusive_for_test();
        db::runtime().block_on(async {
            let service = SettingService::in_memory().await.unwrap();
            assert_eq!(
                service.load_terminal_font().await.unwrap(),
                TerminalFont::default(),
                "没写过就是跟随主题"
            );

            let picked = TerminalFont {
                family: "JetBrains Mono".into(),
                size: 16.,
            };
            service.save_terminal_font(picked.clone()).await.unwrap();
            assert_eq!(service.load_terminal_font().await.unwrap(), picked);
            assert_eq!(font::global(), picked, "缓存也一起换了");
        });
    }
}
