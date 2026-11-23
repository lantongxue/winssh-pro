//! 一次性命令通路的测试：整形（解码、截断、退出码）是纯函数，直接盯；
//! 真跑一段脚本要本机装着 WSL，按项目规矩标 `#[ignore]`，用
//! `WSL_TEST_DISTRO=Ubuntu-24.04 cargo test -p winssh-wslfs -- --ignored` 跑。

#![cfg(windows)]

use std::os::windows::process::ExitStatusExt as _;
use std::time::Duration;

use super::*;

fn output(stdout: &[u8], stderr: &[u8], code: u32) -> std::process::Output {
    std::process::Output {
        status: std::process::ExitStatus::from_raw(code),
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
    }
}

/// 发行版里程序写的是 UTF-8（脚本钉了 LC_ALL=C），原样收，退出码带上。
#[test]
fn shape_keeps_utf8_stdout_and_exit_code() {
    let shaped = shape(output("##hz\n100 4\n".as_bytes(), b"", 0));
    assert_eq!(shaped.stdout, "##hz\n100 4\n");
    assert_eq!(shaped.stderr, "");
    assert_eq!(shaped.exit_status, Some(0));
}

/// wsl.exe 自己的报话（发行版不存在之类）是 UTF-16LE —— 按 UTF-8 解出来是
/// 一串带 NUL 的乱码，用户看不懂失败原因。判据与文件那一半共用 decode_text。
#[test]
fn shape_decodes_utf16le_stderr() {
    let text = "找不到指定的发行版";
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let shaped = shape(output(b"", &bytes, 1));
    assert_eq!(shaped.stderr, text);
    assert_eq!(shaped.exit_status, Some(1));
}

/// 跑飞了的命令不能把内存吃光：超过上限的部分丢掉，已收到的仍是好的。
#[test]
fn shape_caps_output_at_the_limit() {
    let big = vec![b'x'; MAX_OUTPUT + 4096];
    let shaped = shape(output(&big, b"", 0));
    assert_eq!(shaped.stdout.len(), MAX_OUTPUT);
}

/// 真机冒烟：脚本经 stdin 进 /bin/sh，POSIX 写法（命令替换、引号）原样生效，
/// /proc 读得出来 —— 监控采集走的就是这条路。
#[test]
#[ignore = "需要本机装有 WSL，并用 WSL_TEST_DISTRO 指定发行版"]
fn live_runs_a_posix_script_in_the_distro() {
    let Ok(distro) = std::env::var("WSL_TEST_DISTRO") else {
        return;
    };
    let script = "x=$(printf 'a b'); printf '%s\\n' \"$x\"; cut -d' ' -f1 /proc/uptime\n";
    let output = winssh_db::runtime()
        .block_on(exec_script(&distro, script, Duration::from_secs(30)))
        .expect("脚本应跑通");
    assert_eq!(output.exit_status, Some(0), "stderr：{}", output.stderr);
    let mut lines = output.stdout.lines();
    assert_eq!(lines.next(), Some("a b"), "命令替换与引号要原样生效");
    let uptime: f64 = lines
        .next()
        .expect("应有 uptime 一行")
        .trim()
        .parse()
        .expect("uptime 是个数");
    assert!(uptime > 0., "/proc/uptime 读出来应是正数：{uptime}");
}

/// 超时要把这一次真收掉（kill_on_drop），不能留一条挂着的采集在后台。
#[test]
#[ignore = "需要本机装有 WSL，并用 WSL_TEST_DISTRO 指定发行版"]
fn live_timeout_kills_the_run() {
    let Ok(distro) = std::env::var("WSL_TEST_DISTRO") else {
        return;
    };
    let error = winssh_db::runtime()
        .block_on(exec_script(&distro, "sleep 30\n", Duration::from_secs(1)))
        .expect_err("超时必须报错");
    let text = format!("{error:#}");
    assert!(text.contains("1"), "实际：{text}");
}
