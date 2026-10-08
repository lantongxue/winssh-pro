//! Windows 构建要补的几件事：把主线程的栈开大；把虚拟网卡要的 `wintun.dll`
//! 摆到 exe 旁边；把 Clash 那一档要的 `mihomo.exe` 也摆过去（后两件见
//! [`copy_wintun`] 与 [`copy_mihomo`]）。图标与版本信息那段 `winres` 不必解释。
//!
//! ## 主线程的栈
//!
//! MSVC 链接器给可执行文件的默认栈是 **1 MiB**（PE 头里的
//! `SizeOfStackReserve`）。Linux 和 macOS 的主线程默认是 8 MiB，所以这个坑
//! 只在 Windows 上踩得到。
//!
//! GPUI 画一帧是把元素树递归走三遍（`request_layout` → `prepaint` → `paint`），
//! 每一层都是实打实的栈帧。这个应用画一次"Dock + 终端标签页"要多少栈是量过的
//! （见 `ui::panels::terminal` 里那几个 `#[gpui_kit::test]`，配合 `RUST_MIN_STACK`
//! 卡出来的）：`opt-level=0` 下 1.0~1.25 MiB，**已经越过 1 MiB 那条线**。
//! 表现是一连主机就 `has overflowed its stack`，而且和写错的代码毫无关系：
//! 界面稍微深一层就翻车，浅一层又好了，很容易一路查到别处去。
//!
//! 那一半已经在 `Cargo.toml` 里治了 —— 自己的 crate 开到 `opt-level=1`，
//! 需求降到 0.38~0.5 MiB。这里管的是另一半：**把余量补上**。0.5 MiB 用掉
//! 1 MiB 的一半，而"界面多嵌一层"就能吃掉两成，那点余量经不起几次改动；
//! 何况为了跟一个 bug 临时把 `opt-level` 调回 0 时，1 MiB 又不够了。
//!
//! 8 MiB 是照着 Linux / macOS 主线程的默认值来的。它只是**保留地址空间**，
//! 物理页按用到多少提交，所以不占内存。这不是给递归 bug 打补丁——树是有限
//! 深的，那几个测试就是钉这件事的——而是把 Windows 的默认值补齐到别的平台
//! 早就给了的水平。
//!
//! 用 `rustc-link-arg-bins` 而不是 `.cargo/config.toml` 里的 `rustflags`：
//! 后者会把参数加到**每一个** crate 的编译上，一改就是整棵依赖树重编；
//! 这个只作用在最终那个可执行文件上，依赖的缓存一个都不掉。

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/app-icon.ico");

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        copy_wintun();
        copy_mihomo();
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        copy_mihomo_macos();
        // macOS 上 broker 是 launchd 守护（见 `src/bin/broker.rs` 的模块文档），
        // `cargo run` / `cargo test` 时它要在 exe 旁边能找到 ——
        // `clash::broker::broker_binary_path` 找的就是这两个位置。
        copy_broker_sidecar();
    }

    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app-icon.ico");
        res.set("ProductName", "WinSSH Pro");
        res.set("FileDescription", "WinSSH Pro - Native High-Performance SSH Client");
        res.set("LegalCopyright", "Copyright (C) 2026 WinSSH Pro Contributors");
        if let Err(e) = res.compile() {
            eprintln!("cargo:warning=Failed to compile Windows resource: {e}");
        }
    }
}

/// 把 `third_party/<架构>/wintun.dll` 拷到这次构建的 exe 旁边。
///
/// EasyTier 的虚拟网卡那一档（`tun` 特性）在 Windows 上靠 Wintun 驱动建网卡，
/// 驱动装在 `wintun.dll` 里。它不是链接期依赖：`tun-easytier` 在建网卡那一刻
/// 才 `LoadLibraryW("wintun.dll")`，所以编译不缺它，缺了也只在切到虚拟网卡时
/// 报错 —— 而且报的是 `The file is not signed.`：dll 找不到时它拿去验签的
/// 路径退回成了 exe 自己。按名字加载的第一站是 exe 所在目录，dll 就得摆在那儿。
///
/// `third_party/` 下那几份与 EasyTier 仓库（`Cargo.toml` 钉的那个 rev）
/// `easytier/third_party/` 里的逐字节相同，都是 Wintun 0.14.1 的官方构建、
/// WireGuard LLC 签名。别换成自己编的：`tun-easytier` 加载前会验签名并核对
/// 签名者，换了就加载不了。
///
/// 拷两处：`target/[<triple>/]<profile>/`（`cargo run` 与发版打包拿的 exe 在
/// 这儿）和它下面的 `deps/`（`cargo test` 起的测试 exe 在这儿）。输出目录没有
/// 环境变量可读，从 `OUT_DIR`（`<profile>/build/winssh-pro-<hash>/out`）往上数
/// 三层是通行做法。内容没变就不重写：应用开着时 dll 被映射着，写不进去。
fn copy_wintun() {
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x86_64",
        Ok("x86") => "i686",
        Ok("aarch64") => "arm64",
        other => {
            println!("cargo:warning=没有 {other:?} 架构的 wintun.dll，虚拟网卡模式在这个目标上用不了");
            return;
        }
    };
    let source = Path::new("third_party").join(arch).join("wintun.dll");
    copy_sidecar(&source, "wintun.dll", true);
}

/// 把 Clash 那一档要的 `mihomo.exe` 拷到 exe 旁边。
///
/// **这份二进制不由我们发。** 它体积大（六十多 MB）、更新快，还是用户按自己的
/// 判断挑的版本，所以放在 `third_party/mihomo/` 里由用户自己放进去，仓库里
/// 不跟踪它。没有它，应用照跑 —— 只有 Clash 那一档会说"没找到 mihomo"
/// （见 `src/clash/runtime.rs` 的 `binary_path`）。
///
/// 正因为它可有可无，这里**读不到源文件时只警告一句就走**，绝不 panic：
/// 一 panic，所有没放这份二进制的人（包括发版流水线上的机器）都编不过。
///
/// 它和 `wintun.dll` 必须落在同一个目录里：mihomo 建虚拟网卡时按名字加载
/// `wintun.dll`，第一站就是它自己所在的那个目录。
fn copy_mihomo() {
    match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        // 手里这份是 Windows 的 x86_64 构建；别的架构上先不摆，让它去说
        // 那句"没找到 mihomo"。
        Ok("x86_64") => {}
        _ => return,
    }
    let source = Path::new("third_party")
        .join("mihomo")
        .join("windows")
        .join("mihomo-windows-amd64.exe");
    copy_sidecar(&source, "mihomo.exe", false);
}

/// macOS 那一份。和 Windows 同一个规矩：用户自己把 `third_party/mihomo/macos/<arch>/mihomo`
/// 放进来，**我们不代它下载、不代它升级**（那是个会改本机网络设置的程序，哪一个版本、
/// 从哪儿来，得用户自己清楚）。CPU 架构按当前构建目标挑：x86_64 拿 `amd64`，aarch64
/// 拿 `arm64` —— mihomo to-date 没发射 arm64 之外的 Apple Silicon 命名。
///
/// 与 Windows 的另一个区别是这边**不带 `.exe` 后缀** —— macOS 的可执行文件
/// 命名习惯；`binary_path` 在 Unix 平台查的也是这个名字。
fn copy_mihomo_macos() {
    let arch_dir = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "amd64",
        Ok("aarch64") => "arm64",
        _ => return,
    };
    let source = Path::new("third_party")
        .join("mihomo")
        .join("macos")
        .join(arch_dir)
        .join("mihomo");
    // Unix 上的 `copy_sidecar` 不会自带 +x —— `fs::write` 用的是默认 0644。
    // 用户从下载站拷过来的那份大概率带 +x（chmod 是常规动作）；保险起见，
    // 这边 source 没 +x 就跳过拷贝，让 `binary_path` 在 mihomo 目录里直接
    // 拿原文件，免得拷过去的"可执行"既不可执行也跑不起来。
    match std::fs::metadata(&source) {
        Ok(meta) => {
            use std::os::unix::fs::PermissionsExt as _;
            if meta.permissions().mode() & 0o111 == 0 {
                println!(
                    "cargo:warning={} 缺少可执行权限，跳过拷贝（先 `chmod +x {}`）",
                    source.display(),
                    source.display()
                );
                return;
            }
        }
        Err(_) => return,
    }
    copy_sidecar(&source, "mihomo", false);
    // `copy_sidecar`.write 用的是默认 0644 —— 这里给它 +x。
    if let Ok(out_dir) = std::env::var("OUT_DIR") {
        // `<profile>/build/winssh-pro-<hash>/out` 往上数三层到 profile；
        // 那里和 `deps/` 各摆一份（与 `copy_sidecar` 同一处讲究）。
        let profile = Path::new(&out_dir)
            .ancestors()
            .nth(3)
            .map(PathBuf::from);
        if let Some(profile) = profile {
            for candidate in [profile.join("mihomo"), profile.join("deps").join("mihomo")] {
                let _ = chmod_plus_x(&candidate);
            }
        }
    }
}

/// macOS 那一份。broker 被 `cargo build` 编出来之后由这里拷到 exe 旁边，
/// 让 `clash::broker::broker_binary_path` 在开发期也能找到。它在打包出来
/// 的 `.app` 里就不该用这个位置 —— 打包脚本（`scripts/bundle-macos.sh`）
/// 会把它放到 `Contents/Library/LaunchServices/` 去，那才是 macOS 上
/// helper 该在的位置（这块的位置约定见 `clash::broker::broker_binary_path`
/// 的注释）。
#[cfg(target_os = "macos")]
fn copy_broker_sidecar() {
    use std::os::unix::fs::PermissionsExt as _;

    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    let Some(profile) = Path::new(&out_dir).ancestors().nth(3).map(PathBuf::from) else {
        return;
    };
    let source = profile.join("winssh-broker");
    // broker 还没编出来（首次构建）就跳过 —— `cargo build` 的依赖顺序会
    // 在下一轮 build.rs 跑之前把它放到位，错过一轮不碍事。
    if !source.is_file() {
        return;
    }
    for candidate in [profile.join("winssh-broker"), profile.join("deps").join("winssh-broker")] {
        // deps 那一份由 cargo 自然产出（`cargo test` 的产物就在那里），
        // 不需要我们拷 —— 只补 profile 根目录下"货车旁边"那一份。
        if candidate == source {
            // 只补 +x。
            if let Ok(meta) = std::fs::metadata(&source) {
                let mut permissions = meta.permissions();
                let current = permissions.mode();
                permissions.set_mode(current | 0o111);
                let _ = std::fs::set_permissions(&source, permissions);
            }
            continue;
        }
        // deps 那一份 cargo 会摆好。我们不拷、覆盖反而会把 cargo 的指纹搞乱。
        let _ = candidate;
    }
}

#[cfg(unix)]
fn chmod_plus_x(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    if !path.is_file() {
        return Ok(());
    }
    let mut permissions = std::fs::metadata(path)?.permissions();
    let mode = permissions.mode();
    permissions.set_mode(mode | 0o111);
    std::fs::set_permissions(path, permissions)
}
#[cfg(not(unix))]
fn chmod_plus_x(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// 把一份随包带的文件拷到这次构建的 exe 旁边。
///
/// 拷两处：`target/[<triple>/]<profile>/`（`cargo run` 与发版打包拿的 exe 在
/// 这儿）和它下面的 `deps/`（`cargo test` 起的测试 exe 在这儿）。输出目录没有
/// 环境变量可读，从 `OUT_DIR`（`<profile>/build/winssh-pro-<hash>/out`）往上数
/// 三层是通行做法。内容没变就不重写：应用开着时那份文件被映射着（dll）或者
/// 正被跑着（exe），写不进去。
///
/// `required` 见 [`copy_mihomo`] 的文档。`rerun-if-changed` 只在源文件真的在
/// 时才发：指向一个不存在的路径，cargo 会当它每次都在变，于是每回构建都要重跑
/// 一遍这个脚本。
fn copy_sidecar(source: &Path, name: &str, required: bool) {
    let bytes = match std::fs::read(source) {
        Ok(bytes) => {
            println!("cargo:rerun-if-changed={}", source.display());
            bytes
        }
        Err(error) if required => panic!("读不到 {}：{error}", source.display()),
        Err(error) => {
            println!(
                "cargo:warning=没有 {}（{error}）：Clash 那一档在这台机器上找不到内核，其余功能不受影响",
                source.display()
            );
            return;
        }
    };

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo 总会给构建脚本 OUT_DIR"));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR 应当形如 <profile>/build/<包>-<hash>/out");
    for dir in [profile_dir.to_path_buf(), profile_dir.join("deps")] {
        let target = dir.join(name);
        if std::fs::read(&target).is_ok_and(|existing| existing == bytes) {
            continue;
        }
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&target, &bytes))
            .unwrap_or_else(|e| panic!("{name} 拷不到 {}：{e}", target.display()));
    }
}
