# 这份 `gpui-pre-macos` 与上游的差别

`vendor/gpui-pre-macos` 逐字来自 crates.io 的 **gpui-pre-macos 0.3.9**
（Zed 的 `gpui_macos` 的快照，Apache-2.0，`LICENSE-APACHE` 与
`Cargo.toml.orig` 原样留着），`Cargo.toml` 里的 `[patch.crates-io]` 指着它。
版本号必须和 gpui-kit 钉的 `=0.3.9` 一致，patch 才会生效。

它是 `vendor/gpui-pre` 那份补丁二（**窗口隐藏**）的 macOS 一半。为什么要有
这件事、接口长什么样，见 `vendor/gpui-pre/PATCHES.md`。

## 为什么这份也要一起 vendor

Windows 那一半（`vendor/gpui-pre-windows`）早就并进来了；macOS 和 Linux 这两份
本来不在 vendor 里，靠 crates.io 的原版。补丁二给 `PlatformWindow` 加的两个方法
**带默认空实现**，所以就算不并这两份也编得过 —— 代价是 macOS 与 Linux 上
"关窗缩托盘"什么也不发生，窗口留在屏幕上。

`Cargo.toml` 里那两行 `[patch.crates-io]` 是跟着这一版一起加的。`gpui-pre-platform`
在非 Windows 的平台上按 target 拉的是 `gpui-macos` / `gpui-linux` 这两个包名，
Cargo 的 patch 是**按包名**顶替的，所以这两个 key 必须写 `gpui-pre-macos` /
`gpui-pre-linux`（包名），不是 `gpui_macos` / `gpui_linux`（lib 名）。

## 怎么落地

**隐藏**走 AppKit 的 `orderOut:`：窗口从屏幕上消失，应用照常跑，AppKit 随后发
`windowDidChangeOcclusionState:`，GPUI 那边照原样翻译成可见性变化，所以
`Window::visibility()` 会如实翻成 `Hidden`。

**显示**走 `orderFront:`，并且先问一句 `isVisible` —— 已经可见的窗口不做任何事
（幂等）。这里刻意**不用** `makeKeyAndOrderFront:`：那个还会抢焦点并成为键窗口，
而"从托盘把窗口唤回来"该不该同时抢焦点是应用的判断（应用可以在
`show_window` 之后再调 `activate_window()`），补丁不给它定死。

两个方法都照这份 crate 里 `zoom` / `activate` 的既有写法：拿 `native_window`、
克隆一个 `closed` 标记、在 `foreground_executor` 上 spawn 一段，再用
`if_window_not_closed` 挡掉窗口已经关掉的情况。AppKit 的调用必须在主线程上，
而这个 executor 就是主线程那个。

`Platform::supports_window_hiding` 在这边恒为 `true`。

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `src/window.rs` | `impl PlatformWindow for MacWindow` 的 `minimize` 之后加 `hide_window`（`orderOut:`）与 `show_window`（`isVisible` 为假才 `orderFront:`）。 |
| `src/platform.rs` | `impl Platform for MacPlatform` 加 `supports_window_hiding` 返回 `true`。 |
| `src/gpui_macos.rs` | crate 根加 `#![allow(deprecated)]`，静音 cocoa 0.26 那 698 条 deprecated（理由见下节）。 |

其余文件与 crates.io 上的 0.3.9 逐字节相同（`diff -r` 对照即可）。改动都带
`Local patch (see PATCHES.md)` 注释，`grep -rn "Local patch" src/` 能列全。

## 那些 deprecated 警告

`cargo check -p gpui-pre-macos` 在本机（macOS）上会刷 **696 条** warning，全是
一回事：cocoa 0.26 把 `base::id`、`base::nil`、`foundation::NSRect` / `NSPoint` /
`NSSize` / `NSString` 这一整套绑定标了 deprecated，提示改用 **objc2**。分布是
`window.rs` 424 条、`platform.rs` 155 条，其余散在 `display.rs`（33）、
`pasteboard.rs`（31）、`gpui_macos.rs`（24）、`events.rs`（22）、
`window_appearance.rs`（7）。其中最多的是 `base::id`（276）与 `base::nil`（107）。

上游的 0.3.9 原样就是这样，不是我们的补丁带进来的。清掉它只有两条路：

1. **迁到 objc2** —— 等于把 macOS platform 层整个重写，脱离这个快照，不干。
2. **整档静音** —— 现在这条路，crate 根一条 `#![allow(deprecated)]`。

选 2 的理由：这份 crate 的价值就在于"和 Zed 快照逐字一致、只多两个窗口显隐方法"，
平台层真出问题时 `diff` 一下上游就能定位。真正的迁移是跟着快照换代一起做的，
不该在这个补丁副本里先开一个头。哪天做迁移，那条 `allow` 要一并去掉 ——
它只是把噪音按下去，没解决任何问题。

## 测试放在哪

没有可单测的纯逻辑 —— 两个方向各是一句 AppKit 调用。这块按 `AGENTS.md` 的调试
规则**手动验证**：点关闭窗口后 `Dock` 里应用还在、托盘图标还在、之前的 SSH
会话没断（`top` 还在跑），从托盘点回来窗口还在原位置。

注意 macOS 上托盘图标必须在主线程、且在 GPUI 的 `run()` 之前建好（NSApplication
的 run loop 就是 GPUI 那个），应用侧的安排见 `src/tray/`。

## gpui-kit 升级 GPUI 时

见 `vendor/gpui-pre/PATCHES.md` 同名一节：换快照时四份平台 crate 一起重做，
照上表把改动重打一遍。

## 改回上游

哪天上游 GPUI 自己有了窗口隐藏的接口，把应用侧（`src/main.rs`、`src/tray/`）
换过去，然后连同 `vendor/gpui-pre`、`vendor/gpui-pre-windows`、
`vendor/gpui-pre-linux` 一起删掉，并去掉 `Cargo.toml` 里对应的四行
`[patch.crates-io]`。
