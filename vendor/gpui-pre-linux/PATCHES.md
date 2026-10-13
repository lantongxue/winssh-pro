# 这份 `gpui-pre-linux` 与上游的差别

`vendor/gpui-pre-linux` 逐字来自 crates.io 的 **gpui-pre-linux 0.3.9**
（Zed 的 `gpui_linux` 的快照，Apache-2.0，`LICENSE-APACHE` 与
`Cargo.toml.orig` 原样留着），`Cargo.toml` 里的 `[patch.crates-io]` 指着它。
版本号必须和 gpui-kit 钉的 `=0.3.9` 一致，patch 才会生效。

它是 `vendor/gpui-pre` 那份补丁二（**窗口隐藏**）的 Linux 一半。为什么要有
这件事、接口长什么样，见 `vendor/gpui-pre/PATCHES.md`。

## 这一档两个后端，只有 X11 做得到

**X11 能做。** `xcb.unmap_window(x_window)` 把窗口从 X 服务器的屏幕上摘掉：
没有任务栏条目、没有窗管边框，但客户端连接还在。配合 `MapNotify` /
`UnmapNotify`（`client.rs` 里已经处理），GPUI 那边的可见性会如实翻面：

- `hide_window` → `unmap_window` + `xcb_flush`
- `show_window` → `map_window` + `xcb_flush`

两次都刷一遍，因为这两个请求没有 reply，不 flush 就只是躺在请求队列里。
`show_window` 这里没做"已经映射就不重复 map"的判断：多 map 一次在 X 协议里是
无副作用的（`MapNotify` 只在状态真的变化时才发），不值得为此读一次状态。

**Wayland 做不到，而且不是实现问题。** `xdg_toplevel` 协议里根本没有
unmap / hide 这类请求 —— 是否显示由合成器说了算，客户端只能
`set_minimized()` 求它一下。这是 Wayland 的既定设计（和全局快捷键、窗口定位
是同一类被砍掉的能力），绕不过去。所以：

- `WaylandWindow` **不实现**这两个方法，停在 trait 的默认空实现上；
- `DisplayConnection::supports_window_hiding` 对 Wayland 返回 `false`（X11 返回
  `true`，Headless 返回 `false`）。

在这种平台上点关闭窗口**不能**缩到托盘 —— 应用侧据此把那条设置收起来，而不是
给用户一个按下去什么也不发生的开关。隐蔽地退回 `minimize` 是更坏的选择：
任务栏图标还在、窗管的行为也不受控，"退到后台还连着"这个承诺兑现不了。

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `src/linux/x11/window.rs` | `impl PlatformWindow for X11Window` 的 `minimize` 之后加 `hide_window`（`unmap_window`）与 `show_window`（`map_window`），各自 `xcb_flush`。 |
| `src/linux/display_connection.rs` | `impl DisplayConnection` 加 `supports_window_hiding`：X11 `true`，Wayland / Headless `false`。 |
| `src/linux/platform.rs` | `impl Platform for LinuxPlatform` 加 `supports_window_hiding`，转发给当前 `DisplayConnection`。 |

其余文件与 crates.io 上的 0.3.9 逐字节相同（`diff -r` 对照即可）。改动都带
`Local patch (see PATCHES.md)` 注释，`grep -rn "Local patch" src/` 能列全。

`display_connection.rs` 里那个 `dispatch!` 宏（按后端展开 `$connection`）在这处
用不上 —— 它把 X11 和 Wayland 绑到同一个 `$connection` 上，而这里要的正是**区分**
两者，所以写成了直接的 `match`，另两个 `#[cfg(feature = …)]` 分支保持和枚举一致。

## 测试放在哪

没有可单测的纯逻辑。这块按 `AGENTS.md` 的调试规则**手动验证**，且要在 X11 会话
下测（Wayland 上本身就做不到）：点关闭窗口后进程还在、托盘图标还在、SSH 会话
没断，从托盘点回来窗口还在。Wayland 下要验证的是反面 —— 设置里那条
"关闭时最小化到托盘"不出现。

## gpui-kit 升级 GPUI 时

见 `vendor/gpui-pre/PATCHES.md` 同名一节：换快照时四份平台 crate 一起重做，
照上表把改动重打一遍。

## 改回上游

哪天上游 GPUI 自己有了窗口隐藏的接口，把应用侧（`src/main.rs`、`src/tray/`）
换过去，然后连同 `vendor/gpui-pre`、`vendor/gpui-pre-windows`、
`vendor/gpui-pre-macos` 一起删掉，并去掉 `Cargo.toml` 里对应的四行
`[patch.crates-io]`。
