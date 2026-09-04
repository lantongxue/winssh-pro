//! `credentials` 表：保险箱里的一条凭据。
//!
//! 三档凭据（账号密码 / SSH 私钥 / API 令牌）共用一张表，用不上的那几列是空串
//! —— 理由见 [`crate::credential::Credential`]：界面上它们是同一张卡片、同一张
//! 表单，拆成三张表只会让"列一遍保险箱"变成三次查询加一次归并。
//!
//! 这张表和 `hosts` 之间**没有外键**：主机存的是凭据的内容本身，不是指向这里的
//! 一个 id（见 [`crate::credential`] 的模块文档）。删掉一条凭据不会牵动任何主机。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "credentials")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub name: String,
    /// 凭据分档：`password` / `private_key` / `token`。
    #[sea_orm(default_value = "password")]
    pub kind: String,
    /// 分组目录，空串表示还没归类。
    #[sea_orm(default_value = "")]
    pub category: String,
    #[sea_orm(default_value = "")]
    pub username: String,
    /// 密码（`password` 档有效），默认空。落库为明文，同 `hosts.password`。
    #[sea_orm(default_value = "")]
    pub password: String,
    /// 私钥算法：`ed25519` / `rsa4096` / `ecdsa-p384`，空串按 Ed25519 读回。
    #[sea_orm(default_value = "")]
    pub algorithm: String,
    #[sea_orm(default_value = "")]
    pub public_key: String,
    /// 私钥 PEM 文本（`private_key` 档有效）。
    #[sea_orm(default_value = "")]
    pub private_key: String,
    /// 解开私钥的口令，空串就是"这把私钥没加密"。
    #[sea_orm(default_value = "")]
    pub passphrase: String,
    /// `SHA256:...` 形式的公钥指纹。
    #[sea_orm(default_value = "")]
    pub fingerprint: String,
    /// 令牌（`token` 档有效）。
    #[sea_orm(default_value = "")]
    pub token: String,
    /// 标签，JSON 编码的一串字符串。空串表示一个都没有。
    #[sea_orm(default_value = "")]
    pub tags: String,
    #[sea_orm(default_value = "")]
    pub notes: String,
    /// 特别关注。
    #[sea_orm(default_value = false)]
    pub favorite: bool,
    /// 建档时间，`YYYY-MM-DD HH:MM:SS`。列表按它倒序排，新建的在最前 ——
    /// 存成这个格式而不是时间戳，是因为它按字典序排就是按时间排，
    /// 而用 sqlite3 打开库文件时也还读得懂。
    #[sea_orm(default_value = "")]
    pub created_at: String,
    #[sea_orm(default_value = "")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
