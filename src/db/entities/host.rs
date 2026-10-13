//! `hosts` 表：一条保存下来的主机连接配置。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "hosts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub group_id: i64,
    pub name: String,
    pub user: String,
    pub hostname: String,
    pub port: u16,
    pub platform: String,
    pub sort_order: i64,
    /// 连接类型：`ssh` / `vnc` / `rdp` / `telnet` / `serial` / `easytier`，默认 `ssh`。
    /// 见 [`crate::host::HostKind`]。
    #[sea_orm(default_value = "ssh")]
    pub kind: String,
    /// 认证方式：`password` / `key` / `agent`，默认 `password`。
    #[sea_orm(default_value = "password")]
    pub auth_method: String,
    /// 密码（`password` 认证时有效），默认空。
    #[sea_orm(default_value = "")]
    pub password: String,
    /// 私钥 PEM 文本（`key` 认证时有效），默认空。
    /// 存的是内容不是路径，理由见 [`crate::host::Host::key_content`]。
    #[sea_orm(default_value = "")]
    pub key_content: String,
    /// 私钥密码（`key` 认证且私钥加密过时有效），默认空。
    /// 空串就是"这把私钥没加密"，见 [`crate::host::Host::key_passphrase`]。
    #[sea_orm(default_value = "")]
    pub key_passphrase: String,
    /// 跳板机主机 id，空表示直连。
    #[sea_orm(nullable)]
    pub jump_host_id: Option<String>,
    /// 监控面板展开时的采集间隔，秒。见 [`crate::host::MonitorIntervals`]。
    #[sea_orm(default_value = 3)]
    pub monitor_interval_secs: u32,
    /// 面板收起后只喂状态栏的那一档采集间隔，秒。
    #[sea_orm(default_value = 8)]
    pub status_interval_secs: u32,
    /// 这台主机上的端口转发规则，JSON 编码的一串
    /// [`crate::forward::PortForward`]。空串表示一条都没有。
    ///
    /// 一列存一个列表而不是另开一张表：这几条规则只在"打开这台主机的会话"
    /// 时整份读出来、整份写回去，没有任何一处要按规则查主机，拆表只会多一次
    /// join 和一套外键。
    #[sea_orm(default_value = "")]
    pub forwards: String,
    /// 主机标签，JSON 编码的一串字符串列表。空串表示没有标签。
    #[sea_orm(default_value = "")]
    pub tags: String,
    /// 这台主机走不走代理，JSON 编码的一份 [`crate::proxy::HostProxy`]。
    /// 空串是出厂档"跟随全局"，见 [`crate::proxy::decode`]。
    #[sea_orm(default_value = "")]
    pub proxy: String,
    /// VNC 画质配置，JSON 编码的一份 [`crate::vnc::VncSettings`]。
    /// 空串是出厂档，见 [`crate::vnc::decode`]。SSH / RDP 主机原样存着但没人读。
    #[sea_orm(default_value = "")]
    pub vnc_settings: String,
    /// RDP 连接配置，JSON 编码的一份 [`crate::rdp::RdpSettings`]。
    /// 空串是出厂档，见 [`crate::rdp::decode`]。SSH / VNC 主机原样存着但没人读。
    #[sea_orm(default_value = "")]
    pub rdp_settings: String,
    /// 串口参数，JSON 编码的一份 [`crate::serial::SerialSettings`]。
    /// 空串是出厂档，见 [`crate::serial::decode`]。别的协议主机原样存着但没人读。
    ///
    /// 设备路径不在这里，它存在 `hostname` 那一列（串口主机的"地址"
    /// 就是设备路径，见 [`crate::host::Host::address`]）。
    #[sea_orm(default_value = "")]
    pub serial_settings: String,
    /// 组网配置，JSON 编码的一份 [`crate::easytier::EasyTierSettings`]。
    /// 空串是出厂档，见 [`crate::easytier::decode`]。别的协议主机原样存着但没人读。
    ///
    /// 网络名称与密钥不在这里：名称存在 `hostname` 那一列（它就是这台
    /// "主机"的地址），密钥存在 `password` 那一列。
    #[sea_orm(default_value = "")]
    pub easytier_settings: String,
    /// S3 连接配置，JSON 编码的一份 [`crate::s3::S3Settings`]。
    /// 空串是出厂档，见 [`crate::s3::decode`]。别的协议主机原样存着但没人读。
    ///
    /// 端点 URL 在 `hostname` 那一列，Access Key 在 `user` 那一列，
    /// Secret Key 在 `password` 那一列。
    #[sea_orm(default_value = "")]
    pub s3_settings: String,
    /// FTP 加密方式，JSON 编码的一份 [`crate::ftp::FtpSettings`]。
    /// 空串是出厂档（明文），见 [`crate::ftp::decode`]。别的协议主机原样存着但没人读。
    #[sea_orm(default_value = "")]
    pub ftp_settings: String,
    /// 连上之后要不要往对端 shell 里装 OSC 133 标记，出厂是不装。
    /// 装的是什么、为什么要用户点头见 [`crate::terminal::shell_integration`]。
    #[sea_orm(default_value = false)]
    pub shell_integration: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::host_group::Entity",
        from = "Column::GroupId",
        to = "super::host_group::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    HostGroup,
}

impl Related<super::host_group::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::HostGroup.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
