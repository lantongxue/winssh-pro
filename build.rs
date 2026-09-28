//! Windows 构建要补的两件事：把主线程的栈开大；把虚拟网卡要的 `wintun.dll`
//! 摆到 exe 旁边（后者见 [`copy_wintun`]）。图标与版本信息那段 `winres` 不必解释。
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
    println!("cargo:rerun-if-changed={}", source.display());
    let dll = std::fs::read(&source)
        .unwrap_or_else(|e| panic!("读不到 {}（虚拟网卡模式要它）：{e}", source.display()));

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo 总会给构建脚本 OUT_DIR"));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR 应当形如 <profile>/build/<包>-<hash>/out");
    for dir in [profile_dir.to_path_buf(), profile_dir.join("deps")] {
        let target = dir.join("wintun.dll");
        if std::fs::read(&target).is_ok_and(|existing| existing == dll) {
            continue;
        }
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&target, &dll))
            .unwrap_or_else(|e| panic!("wintun.dll 拷不到 {}：{e}", target.display()));
    }
}
