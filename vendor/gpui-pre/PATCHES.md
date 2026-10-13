# 这份 `gpui-pre` 与上游的差别

`vendor/gpui-pre` 来自 crates.io 的 **gpui-pre 0.3.9**（Zed 的 `gpui` 的
快照，由 gpui-kit 发布，Apache-2.0，`LICENSE-APACHE` 与
`Cargo.toml.orig` 原样留着），`Cargo.toml` 里的 `[patch.crates-io]` 指着它。
版本号必须和 gpui-kit 钉的 `=0.3.9` 一致，patch 才会生效。

改动只有两件事：**让应用可以给一张光标位图，当真正的系统光标用**；
以及**让应用可以把一扇窗口从屏幕上拿掉而不销毁它**（托盘"关窗缩起来"要它）。
两者的平台那一半分别在 `vendor/gpui-pre-windows`、`vendor/gpui-pre-macos`、
`vendor/gpui-pre-linux`（见各份 PATCHES.md）。

## 为什么要动它

### 补丁一：应用给的光标位图

RDP 远程桌面要像 mstsc 那样显示远端光标：形状、隐藏、热点都跟着远端，
位置跟着本地鼠标、没有一帧延迟，文本 I 形光标这类 AND/XOR 光标在深色背景上
要反色。能做到这些的只有系统光标本身 —— 画进画面的图既做不了反色，也永远
比鼠标慢一帧，而且本地那颗光标还在（两颗光标）。

GPUI 的光标接口只有一张固定的 `CursorStyle` 表（箭头、I 形、手型……），
没有"应用给的位图"，也没有"隐藏"。这件事应用侧绕不开：光标由 GPUI 每帧按
hitbox 决定、再由平台层 `SetCursor`，应用插不进这条路。

没有给 `CursorStyle` 加枚举值：它是 `Copy` + `Serialize` + `JsonSchema` 的公开
枚举，macOS、Linux、Web 三个平台 crate 都对它做了穷举 `match`，加一个变体它们
就编不过，而这三份不在补丁里。所以另开一条并行的路，其他平台一行不用改。

### 补丁二：窗口隐藏

系统托盘要"点 × 缩起来，SSH 会话还连着"。GPUI 手上那条路只有
`minimize_window()` —— 最小化后任务栏图标还在，点一下自己就回来了，语义是
"暂时收起"而不是"退到后台"；真要点 × 就只剩 `remove_window()`，那会把窗口
连同它下面整棵元素树一起拆掉，`TerminalPanel` 一 Drop，PTY 与 SSH 连接跟着
断。所以"关窗缩托盘"在原来这套接口下**必然等于断线**。

要的是第三种：窗口还在、而且还能画，只是不在屏幕上。Windows 的 `SW_HIDE`、
macOS 的 `orderOut:`、X11 的 `unmap_window` 都是这个语义 —— 应用照常跑，
计时器照常响，`cx.spawn` 的任务照常推进，只是没有人看见。

**Wayland 没有这个能力**，这是协议层面的：`xdg_toplevel` 压根没有 unmap/hide
这类请求，客户端能做的只有 `set_minimized()` 求合成器，之后由它说了算。所以
那一档如实回答 `supports_window_hiding() == false`，应用据此把托盘菜单里那条
"关闭时最小化到托盘"藏起来，而不是给用户一个做不到的开关。

## 接口

### 补丁一

- `CursorImage`（新文件 `src/platform/cursor_image.rs`）：宽高、热点、像素。
  像素三种：`Hidden`（藏起光标）、`Bgra`（带直通 alpha）、`Masked`
  （AND 掩码 + XOR 颜色，经典的 Windows 光标语义，反色靠它）。
- `Window::set_cursor_image(&image, fallback, &hitbox)`：和 `set_cursor_style`
  一样在 paint 阶段按 hitbox 登记，优先级规则完全相同 —— 离开这块区域、被
  菜单或滚动条盖住时，GPUI 照常换成那边登记的光标。
- `Platform::set_cursor_image` / `Platform::supports_cursor_images`：带默认实现
  （不支持）。不支持的平台用 `fallback` 那个样式；应用可以问
  `App::supports_cursor_images` 自己备一个画进画面的替身。

### 补丁二

- `PlatformWindow::hide_window` / `PlatformWindow::show_window`：都带**默认空实现**，
  所以三份没打补丁的平台（Web、以及任何将来的平台）一行不用改，编得过，只是
  隐藏请求过去什么也不发生。`show_window` 幂等 —— 已经可见时什么都不做。
- `Window::hide_window` / `Window::show_window`：转发到上面两个。
- `Platform::supports_window_hiding` / `App::supports_window_hiding`：这个平台到底
  能不能真的把窗拿下去。默认 `false`，Windows / macOS / X11 报 `true`。

**没有做的选择**：没有让 `hide_window` 顺带 `activate`（唤回由应用自己决定要不要
抢焦点），也没有在隐藏时停掉 GPUI 的帧循环（隐藏的窗口本来就不消费 `WM_PAINT`，
Windows 侧 vsync 线程也已经在跳过不可见窗口的 frame signal；应用侧真需要省那点
排版开销，自己看 `Window::visibility()` 短路即可）。

## 逐文件改动

### 补丁一

| 文件 | 改了什么 |
| --- | --- |
| `src/platform/cursor_image.rs` | 新文件：`CursorImage`、`CursorPixels`、`CursorImageId`（进程内唯一，平台按它缓存原生光标）。 |
| `src/platform.rs` | `mod cursor_image` 与 `pub use`；`Platform` trait 加两个带默认实现的方法。 |
| `src/window.rs` | `CursorStyleRequest` 多一个 `image` 字段；新增 `Window::set_cursor_image`；`Frame::cursor_style` 改为返回整条请求（原来只返回样式）；`reset_cursor_style` 先试请求里的图，平台不接再用样式。`use crate::{…}` 多引一个 `CursorImage`（rustfmt 重排了那一段）。 |
| `src/app.rs` | 新增 `App::supports_cursor_images`。 |

### 补丁二

| 文件 | 改了什么 |
| --- | --- |
| `src/platform.rs` | `PlatformWindow` trait 在 `minimize` 后加 `hide_window` / `show_window`（默认空实现）；`Platform` trait 加 `supports_window_hiding`（默认 `false`）。 |
| `src/window.rs` | 新增 `Window::hide_window` / `Window::show_window`，转发到 `self.platform_window`。 |
| `src/app.rs` | 新增 `App::supports_window_hiding`。 |

### 构建警告

| 文件 | 改了什么 |
| --- | --- |
| `src/profiler/journal.rs` | `finished()` 里那条 `fetch_update` 前加 `#[allow(deprecated)]`。新版 stable 把它改名成 `try_update` 并标了 deprecated；这份 crate 是 path 依赖，cargo 不替它压 lint，每次编译都刷一条。不直接换成 `try_update`：那会把能编这份 crate 的最低 Rust 版本抬上去，而上游快照本身没这个要求。 |

另外**删掉了 `examples/`**（4.5 MB，主要是示例图片）。作为依赖时 Cargo 不编
示例，`Cargo.toml` 里那些 `[[example]]` 条目只在编示例时才会去找文件。其余文件
与 crates.io 上的 0.3.9 逐字节相同（`diff -r` 对照即可）。

## 测试放在哪

gpui-pre 自己的 lib 测试离开 zed 仓库编不过（`src/svg_renderer.rs` 在
`cfg(test)` 下 `include_bytes!` 了 zed 仓库里的字体），crates.io 上的原版也是
如此；删掉 `examples/` 之后，`cfg(test)` 下引用示例图片的那一处也找不到文件，
同样只影响这份测试。所以这两处补丁的覆盖在别处：原生光标的生成与回收在
`vendor/gpui-pre-windows` 的单测里，`set_cursor_image` 在 paint 阶段的登记在
应用的 `src/ui/elements/frame.rs`（`every_cursor_form_draws`）里；窗口隐藏没有
可单测的纯逻辑（三个平台各自是一句系统调用），由人工按 `AGENTS.md` 的调试规则
手动验证，应用侧只测它自己的决策（`src/tests/ui/tray.rs`）。

## gpui-kit 升级 GPUI 时

gpui-kit 换一版 `gpui-pre` 快照后，这份补丁的版本号就对不上了：Cargo 会提示
patch 未使用、退回 crates.io 的原版，而应用用到的 `set_cursor_image` /
`hide_window` 会直接编译报错 —— 不会静默丢功能。到时候：

1. 从 crates.io 取新版 `gpui-pre` 与四份平台 crate 的源码，替换这五个目录
   （`gpui-pre-windows`、`gpui-pre-macos`、`gpui-pre-linux`）；
2. 照上表把改动重打一遍（改动都带 `Local patch (see PATCHES.md)` 注释，
   在旧目录里 grep 一下就能列全），删掉 `examples/`；
3. 更新各份 PATCHES.md 里的版本号。

## 改回上游

- 补丁一：哪天上游 GPUI 自己有了"应用光标位图"的接口，把应用侧
  （`src/ui/elements/frame.rs`、`src/ui/panels/rdp.rs`）换过去。
- 补丁二：哪天上游有了窗口隐藏的接口，把应用侧（`src/main.rs`、`src/tray/`）
  换过去。
- 两处都换完之后，删掉本目录与 `vendor/gpui-pre-macos`、`vendor/gpui-pre-linux`，
  以及 `Cargo.toml` 里对应的四行 `[patch.crates-io]`。
