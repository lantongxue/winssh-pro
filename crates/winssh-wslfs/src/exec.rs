//! 在 WSL 发行版里跑一次性命令：监控采集脚本与 `kill` 信号的通路。
//!
//! 与 SSH 那一档（`winssh_ssh::exec`）同形：脚本喂给 `/bin/sh` 的**标准输入**，
//! 不作为 argv 送出 —— `wsl.exe -e` 后面那串要过 Windows 命令行解析与 wsl 自己
//! 的转义两道手，`$(...)`、引号这些 POSIX 写法经不起；stdin 只有一道，字节原样
//! 到 sh。`-e` 直接 exec 指定的程序，不经发行版的登录 shell，fish/csh 也插不上手。
//!
//! 子进程用 `tokio::process` 起在应用级 runtime 上（[`winssh_db::run`] 内部跳过去）：
//! 超时靠 `kill_on_drop` 真把 wsl.exe 收掉，而不是留一条挂着的采集在后台。
//! 被收掉的只是 Windows 这侧的中继，发行版里那个 sh 读到管道断开会自己退出。

use std::time::Duration;

use anyhow::{Result, bail};
#[cfg(windows)]
use anyhow::Context as _;
use rust_i18n::t;

/// 一次命令最多收多少字节。与 SSH 采集同一档：正常一次采集不到 64 KiB，
/// 进程管理那一页要全量进程时也就几百 KiB，这个上限兜的是"跑飞了"。
#[cfg(windows)]
const MAX_OUTPUT: usize = 1 << 20;

/// 一条一次性命令的执行结果。形状与 `winssh_ssh::exec::ExecOutput` 一致，
/// 服务层把两档后端归一成同一种读数再交给解析。
#[derive(Clone, Debug, Default)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    /// 进程的退出码。拿不到（被信号收走之类）时是 `None`。
    pub exit_status: Option<u32>,
}

/// 把一段 POSIX sh 脚本送进发行版执行，收集 stdout / stderr 与退出码。
///
/// `timeout` 到点就放弃这一次：发行版冷启动、磁盘 hang 都可能让命令迟迟不回，
/// 界面的轮询任务不能一直挂着。内部会跳到应用级 tokio runtime 上跑，调用方
/// 在哪个执行器里都行。
pub async fn exec_script(distro: &str, script: &str, timeout: Duration) -> Result<ExecOutput> {
    #[cfg(not(windows))]
    {
        let _ = (distro, script, timeout);
        bail!("{}", t!("wslfs.err.unsupported"));
    }
    #[cfg(windows)]
    {
        let distro = distro.to_owned();
        let script = script.to_owned();
        winssh_db::run(async move { run(&distro, &script, timeout).await }).await
    }
}

#[cfg(windows)]
async fn run(distro: &str, script: &str, timeout: Duration) -> Result<ExecOutput> {
    // 与 WslFs::run_wsl 同一处讲究：采个读数而已，别让控制台黑框闪一下。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut command = tokio::process::Command::new("wsl.exe");
    command
        .arg("-d")
        .arg(distro)
        .args(["-e", "/bin/sh"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = command.spawn().with_context(|| {
        t!("wslfs.err.spawn", distro => distro.to_owned()).to_string()
    })?;

    // 脚本写进 stdin 就关（EOF），sh 读到 EOF 才开始收尾。写失败不在这儿报：
    // sh 没起来（发行版残缺、/bin/sh 不在）时真正的原因在 stderr 与退出码里，
    // 让 wait_with_output 把它们收回来，比一句 broken pipe 有用。
    let mut stdin = child
        .stdin
        .take()
        .context("wsl.exe 的 stdin 管道没开出来")?;
    let script = script.to_owned();
    let writer = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt as _;
        let _ = stdin.write_all(script.as_bytes()).await;
        let _ = stdin.shutdown().await;
    });

    let outcome = tokio::time::timeout(timeout, child.wait_with_output()).await;
    writer.abort();
    let output = match outcome {
        Ok(result) => result.with_context(|| {
            t!("wslfs.err.spawn", distro => distro.to_owned()).to_string()
        })?,
        // 超时这一刻 child 被丢掉，kill_on_drop 把 wsl.exe 收走。
        Err(_) => bail!(
            "{}",
            t!(
                "wslfs.exec.timeout",
                distro => distro.to_owned(),
                seconds => timeout.as_secs(),
            )
        ),
    };
    Ok(shape(output))
}

/// 把子进程的原始输出整形成 [`ExecOutput`]：截到上限再解码。
///
/// 解码走 [`crate::decode_text`] 那套判据：wsl.exe 自己的报话（发行版不存在、
/// 代理配置提醒）是 UTF-16LE，发行版里程序写的是 UTF-8（脚本钉了 `LC_ALL=C`）。
/// 截断按字节切，可能切断一个字符 —— 解码那边是 lossy 的，切坏的最多变成替换符。
#[cfg(windows)]
fn shape(output: std::process::Output) -> ExecOutput {
    fn cap(bytes: &[u8]) -> &[u8] {
        &bytes[..bytes.len().min(MAX_OUTPUT)]
    }
    ExecOutput {
        stdout: crate::decode_text(cap(&output.stdout)),
        stderr: crate::decode_text(cap(&output.stderr)),
        exit_status: output.status.code().map(|code| code as u32),
    }
}

#[cfg(test)]
#[path = "tests/exec.rs"]
mod tests;
