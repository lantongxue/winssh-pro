//! 应用级偏好服务：`settings` 表那些"不属于某台主机、某条凭据"的设置。
//!
//! 现在有六项 —— 全局代理（[`crate::proxy::GlobalProxy`]）、终端字体
//! （[`crate::terminal::font::TerminalFont`]）、终端配色
//! （[`crate::terminal::scheme`]）、终端光标闪烁（[`crate::terminal::cursor`]）、
//! ZMODEM 收发（[`crate::zmodem::config`]）
//! 与智能助手的供应商配置
//! （[`crate::assistant::AssistantConfig`]）。它们的读法一样刁钻：
//!
//! - **设置窗口**在 gpui 线程上读写它们，要的是立刻拿得到、改完立刻画出来；
//! - **拨号**发生在 tokio 那一侧（[`crate::ssh::connection`]），手上没有 `App`，
//!   而且每开一条会话就要读一次；**终端字体与配色**更极端，每一帧都要读一次。
//!
//! 所以库是正本，[`crate::proxy`]、[`crate::terminal::font`] 与
//! [`crate::terminal::scheme`] 各留一份进程级缓存：启动时 [`Self::prime`] 灌一次，
//! 之后每次 `save_*` 两边一起写。少了缓存的话，要么让拨号那条路去查库（多一次
//! await 与一条失败路径），要么让界面每帧查库 —— 两样都不划算。

use anyhow::{Context as _, Result};
use gpui_kit::SharedString;
use sea_orm::DatabaseConnection;

use crate::{
    assistant::{AssistantConfig, config as assistant_config},
    backup::{self, BackupConfig},
    db,
    proxy::{self, GlobalProxy},
    repository::SettingRepository,
    terminal::{
        cursor,
        font::{self, TerminalFont},
        right_click::{self, RightClickMode},
        scheme,
    },
    zmodem::config as zmodem_config,
};

/// 全局代理在 `settings` 表里的键。
const GLOBAL_PROXY_KEY: &str = "proxy.global";
/// 终端字体在 `settings` 表里的键。
const TERMINAL_FONT_KEY: &str = "terminal.font";
/// 终端配色方案在 `settings` 表里的键。存的是方案 id 那一个词。
const TERMINAL_SCHEME_KEY: &str = "terminal.scheme";
/// 智能助手供应商配置在 `settings` 表里的键。存的是一份 JSON。
const ASSISTANT_CONFIG_KEY: &str = "assistant.config";
/// 终端光标闪烁在 `settings` 表里的键。存的是 "true" 或 "false"。
const CURSOR_BLINK_KEY: &str = "terminal.cursor_blink";
/// 终端右键行为在 `settings` 表里的键。存的是 "menu" 或 "paste"。
const TERMINAL_RIGHT_CLICK_KEY: &str = "terminal.right_click";
/// 关窗缩托盘在 `settings` 表里的键。存的是 "true" 或 "false"。
const CLOSE_TO_TRAY_KEY: &str = "tray.close_to_tray";
/// 数据备份同步配置在 `settings` 表里的键。存的是一份 JSON。
const BACKUP_CONFIG_KEY: &str = "backup.config";
/// ZMODEM 收发偏好在 `settings` 表里的键。存的是一份 JSON。
const ZMODEM_CONFIG_KEY: &str = "terminal.zmodem";
/// Clash 那一档 TUN 的偏好在 `settings` 表里的键。存的是一份 JSON
/// （[`crate::clash::tun_settings::TunSettings`]）。
const TUN_SETTINGS_KEY: &str = crate::clash::tun_settings::SETTING_KEY;
/// 自动更新偏好在 `settings` 表里的键。存的是一份 JSON
/// （[`crate::update::UpdatePreferences`]）。
const UPDATE_PREFERENCES_KEY: &str = "update.preferences";

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
    /// 同步阻塞一次：这一步在开窗之前，读的是三行；而它必须在第一条会话
    /// 建起来之前完成 —— 晚一步的话，开机就自动连上的那些主机会以"没有
    /// 全局代理"直连出去，而用户明明配过；终端字体与配色晚一步则是第一帧
    /// 用出厂档画完再跳一下。
    ///
    /// 读不出来只记一行日志：库坏了不该让应用开不起来，界面照常打得开，
    /// 用户在设置里重新配一次就修好了。
    pub fn prime(&self) {
        let this = self.clone();
        let loaded = db::runtime().block_on(async move {
            (
                this.load_global_proxy().await,
                this.load_terminal_font().await,
                this.load_terminal_scheme().await,
                this.load_assistant_config().await,
                this.load_cursor_blink().await,
                this.load_backup_config().await,
                this.load_zmodem_config().await,
                this.load_tun_settings().await,
                this.load_close_to_tray().await,
                this.load_update_preferences().await,
                this.load_terminal_right_click().await,
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
        match loaded.2 {
            Ok(id) => scheme::set_global(&id),
            Err(error) => log::warn!("终端配色读不出来，这次运行跟随界面主题：{error:#}"),
        }
        match loaded.3 {
            Ok(config) => assistant_config::set_global(config),
            Err(error) => log::warn!("助手配置读不出来，这次运行按未配置处理：{error:#}"),
        }
        match loaded.4 {
            Ok(blink) => cursor::set_global(blink),
            Err(error) => log::warn!("光标闪烁读不出来，这次运行按出厂档处理：{error:#}"),
        }
        match loaded.5 {
            Ok(config) => backup::set_global(config),
            Err(error) => log::warn!("备份同步配置读不出来，这次运行按出厂档处理：{error:#}"),
        }
        match loaded.6 {
            Ok(config) => zmodem_config::set_global(config),
            Err(error) => log::warn!("ZMODEM 设置读不出来，这次运行按出厂档处理：{error:#}"),
        }
        match loaded.7 {
            Ok(settings) => crate::clash::tun_settings::set_current(settings),
            Err(error) => log::warn!("Clash TUN 偏好读不出来，这次运行按出厂档处理：{error:#}"),
        }
        match loaded.8 {
            Ok(close_to_tray) => crate::tray::set_global(close_to_tray),
            Err(error) => log::warn!("关窗缩托盘的开关读不出来，这次运行按出厂档处理：{error:#}"),
        }
        match loaded.9 {
            Ok(preferences) => crate::update::set_global(preferences),
            Err(error) => log::warn!("更新偏好读不出来，这次运行按出厂档处理：{error:#}"),
        }
        match loaded.10 {
            Ok(mode) => right_click::set_global(mode),
            Err(error) => log::warn!("终端右键行为读不出来，这次运行按右键菜单处理：{error:#}"),
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

    /// 库里那份终端配色方案的 id。没写过、或者认不出来就是空串（跟随界面主题）。
    pub async fn load_terminal_scheme(&self) -> Result<SharedString> {
        let this = self.clone();
        db::run(async move {
            let raw = this.repo.get(&this.db, TERMINAL_SCHEME_KEY).await?;
            Ok(scheme::decode(&raw.unwrap_or_default()))
        })
        .await
    }

    /// 写回终端配色方案，并同时换掉进程里那份缓存。
    ///
    /// 缓存先换，理由和 [`Self::save_global_proxy`] 一样。
    pub async fn save_terminal_scheme(&self, id: SharedString) -> Result<()> {
        scheme::set_global(&id);
        let this = self.clone();
        // 存的是规范化之后那一个：认不出来的 id 落库还是那串字，下次启动
        // 又要认一次、又要写一行日志。
        let encoded = scheme::encode(&scheme::global());
        db::run(async move {
            this.repo
                .set(&this.db, TERMINAL_SCHEME_KEY, &encoded)
                .await
                .map_err(Into::into)
        })
        .await
    }

    /// 库里那份智能助手配置。没写过就是出厂档（未配置、每条命令都送审）。
    pub async fn load_assistant_config(&self) -> Result<AssistantConfig> {
        let this = self.clone();
        db::run(async move {
            let raw = this.repo.get(&this.db, ASSISTANT_CONFIG_KEY).await?;
            Ok(assistant_config::decode(&raw.unwrap_or_default()))
        })
        .await
    }

    /// 写回智能助手配置，并同时换掉进程里那份缓存。
    ///
    /// 缓存先换，理由和 [`Self::save_global_proxy`] 一样：设置页改完这一格，
    /// 助手抽屉那边下一次"发送"就该按新的走。
    pub async fn save_assistant_config(&self, config: AssistantConfig) -> Result<()> {
        assistant_config::set_global(config.clone());
        let this = self.clone();
        let encoded = assistant_config::encode(&config);
        db::run(async move {
            this.repo
                .set(&this.db, ASSISTANT_CONFIG_KEY, &encoded)
                .await
                .map_err(Into::into)
        })
        .await
    }

    /// 库里那份光标闪烁设置。没写过就是出厂档（闪烁）。
    pub async fn load_cursor_blink(&self) -> Result<bool> {
        let this = self.clone();
        db::run(async move {
            let raw = this.repo.get(&this.db, CURSOR_BLINK_KEY).await?;
            Ok(cursor::decode(&raw.unwrap_or_default()))
        })
        .await
    }

    /// 写回光标闪烁设置，并同时换掉进程里那份缓存。
    ///
    /// 缓存先换，理由和 [`Self::save_global_proxy`] 一样。
    pub async fn save_cursor_blink(&self, blink: bool) -> Result<()> {
        cursor::set_global(blink);
        let this = self.clone();
        let encoded = cursor::encode(blink);
        db::run(async move {
            this.repo
                .set(&this.db, CURSOR_BLINK_KEY, &encoded)
                .await
                .map_err(Into::into)
        })
        .await
    }

    /// 库里那份终端右键行为。没写过就是出厂档（右键菜单）。
    pub async fn load_terminal_right_click(&self) -> Result<RightClickMode> {
        let this = self.clone();
        db::run(async move {
            let raw = this.repo.get(&this.db, TERMINAL_RIGHT_CLICK_KEY).await?;
            Ok(right_click::decode(&raw.unwrap_or_default()))
        })
        .await
    }

    /// 写回终端右键行为，并同时换掉进程里那份缓存。
    pub async fn save_terminal_right_click(&self, mode: RightClickMode) -> Result<()> {
        right_click::set_global(mode);
        let this = self.clone();
        let encoded = right_click::encode(mode);
        db::run(async move {
            this.repo
                .set(&this.db, TERMINAL_RIGHT_CLICK_KEY, &encoded)
                .await
                .map_err(Into::into)
        })
        .await
    }

    /// 库里那份关窗缩托盘的开关。没写过就是出厂档（开）。
    pub async fn load_close_to_tray(&self) -> Result<bool> {
        let this = self.clone();
        db::run(async move {
            let raw = this.repo.get(&this.db, CLOSE_TO_TRAY_KEY).await?;
            Ok(crate::tray::decode(&raw.unwrap_or_default()))
        })
        .await
    }

    /// 写回关窗缩托盘的开关，并同时换掉进程里那份缓存。
    ///
    /// 缓存先换：用户下一次点关闭可能就在这一帧之后，落库那条路慢一拍不要紧。
    pub async fn save_close_to_tray(&self, close_to_tray: bool) -> Result<()> {
        crate::tray::set_global(close_to_tray);
        let this = self.clone();
        let encoded = crate::tray::encode(close_to_tray);
        db::run(async move {
            this.repo
                .set(&this.db, CLOSE_TO_TRAY_KEY, &encoded)
                .await
                .map_err(Into::into)
        })
        .await
    }

    /// 库里那份数据备份同步配置。没写过就是出厂档（未开启）。
    pub async fn load_backup_config(&self) -> Result<BackupConfig> {
        let this = self.clone();
        db::run(async move {
            let raw = this.repo.get(&this.db, BACKUP_CONFIG_KEY).await?;
            match raw {
                Some(json) if !json.trim().is_empty() => {
                    serde_json::from_str(&json).context("解析备份配置失败")
                }
                _ => Ok(BackupConfig::default()),
            }
        })
        .await
    }

    /// 写回数据备份同步配置，并同时换掉进程里那份缓存。
    pub async fn save_backup_config(&self, config: BackupConfig) -> Result<()> {
        backup::set_global(config.clone());
        let this = self.clone();
        let encoded = serde_json::to_string(&config)?;
        db::run(async move {
            this.repo
                .set(&this.db, BACKUP_CONFIG_KEY, &encoded)
                .await
                .map_err(Into::into)
        })
        .await
    }

    /// 库里那份 ZMODEM 偏好。没写过就是出厂档（开着、接收先问、跟随系统
    /// 下载目录）。
    pub async fn load_zmodem_config(&self) -> Result<zmodem_config::ZmodemConfig> {
        let this = self.clone();
        db::run(async move {
            let raw = this.repo.get(&this.db, ZMODEM_CONFIG_KEY).await?;
            Ok(zmodem_config::decode(&raw.unwrap_or_default()))
        })
        .await
    }

    /// 写回 ZMODEM 偏好，并同时换掉进程里那份缓存。
    ///
    /// 缓存先换，理由同 [`Self::save_global_proxy`]：改完这一格，下一次
    /// 认出 rz/sz 就该按新的走。
    pub async fn save_zmodem_config(&self, config: zmodem_config::ZmodemConfig) -> Result<()> {
        zmodem_config::set_global(config.clone());
        let this = self.clone();
        let encoded = zmodem_config::encode(&config);
        db::run(async move {
            this.repo
                .set(&this.db, ZMODEM_CONFIG_KEY, &encoded)
                .await
                .map_err(Into::into)
        })
        .await
    }

    /// 库里那份 Clash TUN 偏好。没写过就是出厂档（system / winssh-tun /
    /// 1500 / 路由与出口自动）。
    pub async fn load_tun_settings(&self) -> Result<crate::clash::TunSettings> {
        let this = self.clone();
        db::run(async move {
            let raw = this.repo.get(&this.db, TUN_SETTINGS_KEY).await?;
            match raw {
                Some(json) if !json.trim().is_empty() => {
                    serde_json::from_str(&json).context("解析 Clash TUN 偏好失败")
                }
                _ => Ok(crate::clash::TunSettings::default()),
            }
        })
        .await
    }

    /// 写回 Clash TUN 偏好，并同时换掉进程里那份缓存（`tun_settings::set_current`）。
    /// 下一次起实例 / 下一次 `Reload` 就按新的拼 `config.yaml`。
    pub async fn save_tun_settings(&self, settings: crate::clash::TunSettings) -> Result<()> {
        crate::clash::tun_settings::set_current(settings.clone());
        let this = self.clone();
        let encoded = serde_json::to_string(&settings)?;
        db::run(async move {
            this.repo
                .set(&this.db, TUN_SETTINGS_KEY, &encoded)
                .await
                .map_err(Into::into)
        })
        .await
    }

    /// 库里那份自动更新偏好。没写过就是出厂档（自动检查开着、没跳过任何版本）。
    pub async fn load_update_preferences(&self) -> Result<crate::update::UpdatePreferences> {
        let this = self.clone();
        db::run(async move {
            let raw = this.repo.get(&this.db, UPDATE_PREFERENCES_KEY).await?;
            Ok(crate::update::decode(&raw.unwrap_or_default()))
        })
        .await
    }

    /// 写回自动更新偏好，并同时换掉进程里那份缓存。
    ///
    /// 缓存先换：设置里改完这一格，下一次启动检查就该按新的走，而落库是异步的。
    pub async fn save_update_preferences(
        &self,
        preferences: crate::update::UpdatePreferences,
    ) -> Result<()> {
        crate::update::set_global(preferences.clone());
        let this = self.clone();
        let encoded = crate::update::encode(&preferences);
        db::run(async move {
            this.repo
                .set(&this.db, UPDATE_PREFERENCES_KEY, &encoded)
                .await
                .map_err(Into::into)
        })
        .await
    }
}

#[cfg(test)]
#[path = "tests/service/setting_service.rs"]
mod tests;
