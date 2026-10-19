//! SSH 主机上 SFTP 那条通道怎么开：标准子系统、sudo 提权，还是一条自定义命令。
//!
//! 与 [`crate::settings::ftp`] 同一套写法：`serde` 序列化成 JSON 落库，出厂档
//! 写空串。账号、密码复用 `hosts` 表已有的那几列 —— sudo 要的密码默认就是
//! 登录密码，这里只放"怎么起 sftp-server"这一样。
//!
//! 真正按这份设置开通道的是 `winssh_ssh::sftp`；这里只是数据。

use serde::{Deserialize, Serialize};

/// SFTP 通道的三种开法。
///
/// - **子系统（Subsystem）**：`subsystem sftp`，sshd 替你起 sftp-server，
///   权限就是登录账号的权限。出厂档 —— 这一列出现之前存下的每台主机
///   走的都是它，老库补出来的空串不改变任何人的连法。
/// - **sudo（Sudo）**：`exec` 一段脚本，找到 sftp-server 后用 `sudo -S` 起它，
///   要密码时由客户端喂进去，之后同一条 stdin 交给 SFTP 协议。
///   [`SftpSettings::run_as`] 空着是 root。
/// - **自定义（Custom）**：把 [`SftpSettings::command`] 原样 `exec` 出去（WinSCP
///   的"SFTP 服务器"那一栏）。不喂密码 —— 给只放行 sftp-server 一条命令的
///   NOPASSWD sudoers 用，如 `sudo /usr/libexec/openssh/sftp-server`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SftpLaunchMode {
    #[default]
    Subsystem,
    Sudo,
    Custom,
}

impl SftpLaunchMode {
    /// 下拉里自上而下的顺序。[`Self::from_index`] 按的就是这个顺序。
    pub const ALL: [Self; 3] = [Self::Subsystem, Self::Sudo, Self::Custom];

    pub fn index(self) -> usize {
        match self {
            Self::Subsystem => 0,
            Self::Sudo => 1,
            Self::Custom => 2,
        }
    }

    /// 越界按出厂档算 —— 界面选不出越界的下标，真出现了也不该卡住表单。
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// SSH 主机的 SFTP 开法。进 [`crate::host::HostDraft`]，在主机表单的
/// "SFTP" 那一页里填；SFTP 面板、终端底部的 SFTP 栏与智能助手的传输共用这一份。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SftpSettings {
    pub mode: SftpLaunchMode,
    /// sudo 档切成谁（`sudo -u`）。空串是 root。只在 [`SftpLaunchMode::Sudo`] 下读。
    pub run_as: String,
    /// 自定义档整条照发的命令。只在 [`SftpLaunchMode::Custom`] 下读。
    pub command: String,
}

impl SftpSettings {
    /// 表单收尾时的校验：不合规矩就返回给用户看的那句话的 i18n 键。
    ///
    /// `run_as` 要原样拼进远端的 shell 命令行，所以只认用户名的字符集
    /// （见 [`is_valid_user`]）；自定义档的命令不能空 —— 空命令等于没配。
    pub fn validate(&self) -> Result<(), &'static str> {
        match self.mode {
            SftpLaunchMode::Subsystem => Ok(()),
            SftpLaunchMode::Sudo => {
                let user = self.run_as.trim();
                if user.is_empty() || is_valid_user(user) {
                    Ok(())
                } else {
                    Err("host_form.sftp_run_as_invalid")
                }
            }
            SftpLaunchMode::Custom => match self.command.trim().is_empty() {
                true => Err("host_form.sftp_command_required"),
                false => Ok(()),
            },
        }
    }

    /// sudo 档要切成的用户，root 时为 `None`（`sudo` 不带 `-u`）。
    /// 字符集不合规矩的也按 `None` 处理 —— 表单已经拦过，这里是兜底，
    /// 绝不把一段来路不明的字串拼进远端命令行。
    pub fn sudo_user(&self) -> Option<&str> {
        let user = self.run_as.trim();
        (!user.is_empty() && user != "root" && is_valid_user(user)).then_some(user)
    }
}

/// 能不能当成 `sudo -u` 的参数直接拼进命令行：POSIX 可移植用户名的字符集
/// （字母、数字、`.`、`_`、`-`，不以 `-` 开头），外加 sudo 认的 `#uid` 写法。
/// 不出现空白、引号与 shell 元字符，也就不需要转义。
pub fn is_valid_user(user: &str) -> bool {
    if let Some(uid) = user.strip_prefix('#') {
        return !uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit());
    }
    !user.is_empty()
        && user.len() <= 32
        && !user.starts_with('-')
        && user
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// 把设置写成 `hosts.sftp_settings` 那一列。出厂档写空串，同 [`crate::settings::ftp::encode`]。
pub fn encode(settings: &SftpSettings) -> String {
    if *settings == SftpSettings::default() {
        return String::new();
    }
    serde_json::to_string(settings).unwrap_or_default()
}

/// 从 `hosts.sftp_settings` 那一列读回设置。读不动就当出厂档（标准子系统），
/// 理由同 [`crate::settings::ftp::decode`]：为一段坏 JSON 让整个主机列表打不开不成比例。
pub fn decode(raw: &str) -> SftpSettings {
    if raw.trim().is_empty() {
        return SftpSettings::default();
    }
    match serde_json::from_str(raw) {
        Ok(settings) => settings,
        Err(error) => {
            log::warn!("hosts.sftp_settings 解析失败，这台主机的 SFTP 按标准子系统开：{error}");
            SftpSettings::default()
        }
    }
}

#[cfg(test)]
#[path = "../tests/settings/sftp.rs"]
mod tests;
