//! WSL（Windows Subsystem for Linux）的探测与接入。
//!
//! 标题栏的 Shell 按钮在这台机器上有 WSL 发行版时会多出一列下拉（见
//! [`crate::ui::workspace`]），选中的那一条走的仍是本机 PTY —— 只是把 PTY
//! 里要跑的程序从默认登录 shell 换成 `wsl.exe -d <发行版>`，ConPTY 把它当
//! 一条普通控制台程序接上，里面的 Linux shell 对终端完全透明。

use std::process::Command;

use alacritty_terminal::tty::Shell;

/// 走 WSL 的那条 PTY 要跑的程序与参数。
pub fn shell(distro: &str) -> Shell {
    Shell::new("wsl.exe".into(), vec!["-d".into(), distro.into()])
}

/// 列出这台机器上已注册的 WSL 发行版（`wsl.exe --list --quiet`）。
///
/// 任何一步走不通（没有 wsl.exe、还没装发行版、命令报错）都返回空表：
/// "没有 WSL" 与 "WSL 坏了" 在标题栏是同一个待遇 —— 下拉不出现，按钮
/// 退回原来的独粒样子。
///
/// 调用方要把它放进后台执行器：这一下要起一个子进程，慢的话几百毫秒，
/// 堵在开窗那一帧不值得。
pub fn probe_distros() -> Vec<String> {
    let mut command = Command::new("wsl.exe");
    command.args(["--list", "--quiet"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // 与 clash::runtime、local_file_service 同一处讲究：探个列表而已，
        // 别让控制台黑框闪一下。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    // 装了 wsl.exe 但一个发行版都没有（或 WSL 残缺）时它带着一段
    // 本地化的报话退出，退出码非零 —— 那段话不是发行版名，不能拿去解析。
    if !output.status.success() {
        return Vec::new();
    }
    parse_distros(&output.stdout)
}

/// 解析 `--quiet` 的输出，一行一个发行版，空行丢弃。
///
/// `wsl.exe` 往管道里写字用的是 UTF-16LE（Windows 自家工具的惯例，BOM
/// 时有时无），不是终端上看着的 UTF-8 —— 拿 stdout 的字节直接按 UTF-8
/// 解出来是一串带 NUL 的乱码。所以先认字节形状再解（见 [`decode`]）。
pub fn parse_distros(bytes: &[u8]) -> Vec<String> {
    decode(bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// 把 `wsl.exe` 的 stdout 还原成文本。
///
/// 判据两条：开头是 UTF-16LE 的 BOM（`FF FE`），或者 NUL 的密度高 ——
/// UTF-16 的 ASCII 段每两个字节就有一个 NUL，而正经 UTF-8 文本里
/// NUL 几乎不出现。两条都不满足就当它是 UTF-8。
fn decode(bytes: &[u8]) -> String {
    let bom = bytes.starts_with(&[0xFF, 0xFE]);
    let nuls = bytes.iter().filter(|byte| **byte == 0).count();
    if bom || nuls * 4 > bytes.len() {
        let skip = if bom { 2 } else { 0 };
        let units: Vec<u16> = bytes[skip..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
#[path = "../tests/terminal/wsl.rs"]
mod tests;
