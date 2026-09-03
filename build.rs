//! 只做一件事：把 Windows 上主线程的栈开大。
//!
//! MSVC 链接器给可执行文件的默认栈是 **1 MiB**（PE 头里的
//! `SizeOfStackReserve`）。Linux 和 macOS 的主线程默认是 8 MiB，所以这个坑
//! 只在 Windows 上踩得到。
//!
//! GPUI 画一帧是把元素树递归走三遍（`request_layout` → `prepaint` → `paint`），
//! 每一层都是实打实的栈帧；debug 构建不内联、局部变量也不复用槽位，一层能吃
//! 掉几 KiB。实测这个应用画一次"Dock + 终端标签页"要 1.1~1.2 MiB
//! （见 `ui::panels::terminal` 里那几个 `#[gpui::test]`，配合
//! `RUST_MIN_STACK` 卡出来的），**已经越过 1 MiB 那条线**——表现是一连主机
//! 就 `has overflowed its stack`，而且和写错的代码毫无关系：
//! 界面稍微深一层就翻车，浅一层又好了，很容易一路查到别处去。
//!
//! 所以这里把它抬到 8 MiB，和另外两个平台的主线程对齐。这不是给递归 bug
//! 打补丁——树是有限深的，那几个测试就是钉这件事的——而是把 Windows 的默认值
//! 补齐到别的平台早就给了的水平。
//!
//! 用 `rustc-link-arg-bins` 而不是 `.cargo/config.toml` 里的 `rustflags`：
//! 后者会把参数加到**每一个** crate 的编译上，一改就是整棵依赖树重编；
//! 这个只作用在最终那个可执行文件上，依赖的缓存一个都不掉。

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }
}
