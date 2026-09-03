//! 只做一件事：把 Windows 上主线程的栈开大。
//!
//! MSVC 链接器给可执行文件的默认栈是 **1 MiB**（PE 头里的
//! `SizeOfStackReserve`）。Linux 和 macOS 的主线程默认是 8 MiB，所以这个坑
//! 只在 Windows 上踩得到。
//!
//! GPUI 画一帧是把元素树递归走三遍（`request_layout` → `prepaint` → `paint`），
//! 每一层都是实打实的栈帧。这个应用画一次"Dock + 终端标签页"要多少栈是量过的
//! （见 `ui::panels::terminal` 里那几个 `#[gpui::test]`，配合 `RUST_MIN_STACK`
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

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }
}
