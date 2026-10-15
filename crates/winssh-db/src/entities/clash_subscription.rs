//! `clash_subscriptions` 表：一条 mihomo 订阅的名字、来路与整份正文。
//!
//! 正文（YAML）整份存在这一行里，见 `crate::db` 建表语句上那段理由。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "clash_subscriptions")]
pub struct Model {
    /// 主键，由 [`crate::service::ClashSubscriptionService`] 依据订阅名分配。
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// 显示名，例如 `机场 A`。
    pub name: String,
    /// 订阅地址。手工粘贴或从文件导入的订阅没有地址，是空串。
    #[sea_orm(default_value = "")]
    pub source_url: String,
    /// 订阅正文（Clash 配置 YAML）。空串表示这条订阅还没拉到过内容。
    #[sea_orm(default_value = "")]
    pub config_yaml: String,
    /// 分组标签，筛选用。空串表示未归类。
    #[sea_orm(default_value = "")]
    pub tag: String,
    /// 卡片标识色（十六进制 `#RRGGBB`，或空串走默认）。
    #[sea_orm(default_value = "")]
    pub color: String,
    /// 这份订阅的累计上传字节。`None` 是机场没回这个数。
    pub upload_bytes: Option<i64>,
    /// 累计下行字节。
    pub download_bytes: Option<i64>,
    /// 套餐总字节。`None` 是机场没回这个数。
    pub total_bytes: Option<i64>,
    /// 套餐到期日（`YYYY-MM-DD`）。`None` 是机场没回。
    pub expire_date: Option<String>,
    /// 拉取订阅时报给机场的 User-Agent（机场按它决定回 YAML 还是节点列表）。
    #[sea_orm(default_value = "")]
    pub user_agent: String,
    /// 自动更新频率（小时）。0 是不自动更新。
    #[sea_orm(default_value = 0)]
    pub auto_update_hours: i32,
    /// 用户备注。空串是没有。
    #[sea_orm(default_value = "")]
    pub notes: String,
    #[sea_orm(default_value = "")]
    pub created_at: String,
    /// 最后一次自动拉取尝试的时间（拉取成功或失败都会更新），用于判断
    /// 下次自动更新是否到期。手工导入时也是创建/更新的时刻。
    #[sea_orm(default_value = "")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
