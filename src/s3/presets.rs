//! S3 服务商预设：主机表单与备份页共用的一份"选一家服务商，带出默认配置"。
//!
//! 预设只带三样事实：默认端点地址、默认 Region 与寻址风格。界面上的名字
//! 不在这里 —— 表单走 locale（`host_form.s3_preset_*`），备份页用自己那份
//! 硬编码名（见 [`crate::backup::S3_PRESETS`]）。预设 id 是落库的稳定键，
//! 只能新增、不能改名。
//!
//! 阿里云 OSS / 腾讯云 COS 只认虚拟主机风格寻址，MinIO / 自建服务吃路径
//! 风格（也是 [`S3Addressing::Auto`] 那一档的发法），这一档写在预设里，选中即预填。

use super::settings::S3Addressing;

/// 一家服务商的出厂配置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct S3Preset {
    /// 落库的稳定键：`"aws" | "aliyun" | "tencent" | "qiniu" | "minio" | "custom"`。
    pub id: &'static str,
    /// 默认端点地址。`"custom"` 预设不预填，端点完全由用户自填。
    pub default_endpoint: &'static str,
    /// 默认 Region。空串 = 留空走 [`super::settings::infer_region`] 自动推断。
    pub default_region: &'static str,
    /// 寻址风格。
    pub addressing: S3Addressing,
}

/// Amazon S3。AWS 两种寻址风格都认，按出厂档的路径风格发。
pub const AWS: S3Preset = S3Preset {
    id: "aws",
    default_endpoint: "https://s3.amazonaws.com",
    default_region: "us-east-1",
    addressing: S3Addressing::Auto,
};

/// 阿里云 OSS。端点自带地域，只认虚拟主机风格。
pub const ALIYUN: S3Preset = S3Preset {
    id: "aliyun",
    default_endpoint: "https://oss-cn-hangzhou.aliyuncs.com",
    default_region: "",
    addressing: S3Addressing::VirtualHost,
};

/// 腾讯云 COS。端点自带地域，只认虚拟主机风格。
pub const TENCENT: S3Preset = S3Preset {
    id: "tencent",
    default_endpoint: "https://cos.ap-guangzhou.myqcloud.com",
    default_region: "",
    addressing: S3Addressing::VirtualHost,
};

/// 七牛云 Kodo。
pub const QINIU: S3Preset = S3Preset {
    id: "qiniu",
    default_endpoint: "https://s3.cn-east-1.qiniucs.com",
    default_region: "cn-east-1",
    addressing: S3Addressing::VirtualHost,
};

/// MinIO 一类的自建服务。路径风格，Region 无从推断。
pub const MINIO: S3Preset = S3Preset {
    id: "minio",
    default_endpoint: "http://127.0.0.1:9000",
    default_region: "",
    addressing: S3Addressing::Auto,
};

/// 自定义：不预填任何东西，一切由用户自填。
pub const CUSTOM: S3Preset = S3Preset {
    id: "custom",
    default_endpoint: "",
    default_region: "",
    addressing: S3Addressing::Auto,
};

/// 全部预设。表单下拉按这个顺序摆，下标即选中项。
pub const ALL: [S3Preset; 6] = [CUSTOM, AWS, ALIYUN, TENCENT, QINIU, MINIO];

#[cfg(test)]
#[path = "../tests/s3/presets.rs"]
mod tests;
