# 这份 `gpui-pre-windows` 与上游的差别

`vendor/gpui-pre-windows` 逐字来自 crates.io 的 **gpui-pre-windows 0.3.8**
（Zed 的 `gpui_windows` 的快照，Apache-2.0，`LICENSE-APACHE` 与
`Cargo.toml.orig` 原样留着），`Cargo.toml` 里的 `[patch.crates-io]` 指着它。

它是 `vendor/gpui-pre` 那份补丁的 Windows 一半：把应用给的 `gpui::CursorImage`
做成真正的 Windows 光标。为什么要有这件事、接口长什么样，见
`vendor/gpui-pre/PATCHES.md`。

## 怎么落地

`CreateIconIndirect` 按图的三种像素做出 `HCURSOR`，形式就是远端 Windows 自己的那种：

- **带 alpha**（`Bgra`）：32 位 DIB 带 alpha 通道。有 alpha 时 Windows 不看掩码。
- **AND/XOR 且颜色只有黑白**（`Masked`，文本 I 形光标这类）：不给彩色位图，
  掩码上半是 AND、下半是 XOR 的单色光标。反色由系统完成。
- **AND/XOR 带别的颜色**：彩色位图 alpha 全零，Windows 于是按
  `(屏幕 AND 掩码) XOR 颜色` 合成。
- **隐藏**：没有句柄，`SetCursor(NULL)`，走 GPUI 原来"隐藏光标"那条路。

光标按图的像素尺寸显示，Windows 不缩放这样做出来的光标 —— RDP 画面按物理像素
1:1 绘制，远端的 32×32 光标在本地也是 32×32 个物理像素。

## 两处行为

**句柄回收。** 标准光标是共享的、不用关；这些不是。缓存按图的 id 存句柄、拿着
图的 `Weak`：应用还拿着图就留着（服务器常在几张光标之间来回切，不必重建），
图没了才 `DestroyCursor`。但光标变化是通过 `PostMessage` 传给各窗口的，平台
换了不代表每个窗口都处理完了消息，所以销毁前还要确认**平台和每个窗口**的当前
光标都不是它。

**光标变化立即显示。** 原来窗口收到光标变化时，只有"有光标 ↔ 无光标"切换才立刻
`SetCursor`，其余等下一次 `WM_SETCURSOR`，也就是鼠标下一次移动。远端光标的形状
是在网络往返之后才到的，那时鼠标常常已经停了；按住鼠标拖动时窗口还捕获着鼠标，
系统根本不发 `WM_SETCURSOR`。所以现在只要鼠标在这扇窗口上、上一次
`WM_SETCURSOR` 也是这扇窗口自己定的光标（不是交给系统的缩放边框），光标一变就
立刻 `SetCursor`。这条对标准样式同样生效：GPUI 自己的样式切换也不再晚一次鼠标移动。

## 补丁二：纯 COLR 字体的 emoji 画不出来（国旗空白）

**现象。** 打包一份带全量国旗的彩色 emoji 字体（Twemoji Mozilla，Firefox 在
Windows 上用的那份），国旗一格空白；系统 Segoe UI Emoji 的彩色 emoji 却一直
正常。

**根因。** `raster_bounds` 拿的是**单色轮廓**的 alpha 包围盒
（`GetAlphaTextureBounds`，基于 `glyf`），而纯 COLR 字体的 emoji 底层轮廓是空
的（彩色全在 COLR 层里），返回的是**零尺寸**矩形。这里埋着两层坑：

1. 上游的空矩形判定是 `bounds.right < bounds.left`（假设 DirectWrite 返回
   倒置矩形），零尺寸（`right == left`）判不出来，落进正常分支算出零包围盒；
2. `Window::paint_emoji` 画之前有 `if !raster_bounds.is_zero()` 守卫，零包围
   盒整个字形直接跳过 —— 字体打得再全也只剩空白。

Segoe UI Emoji 之所以能画，是它的 emoji glyph 带黑色轮廓（兼容旧渲染器）。

**修法。** 两处：空矩形判定改成 `right <= left || bottom <= top`；空了且
`params.is_emoji` 时按 `rasterize_color` 同一套枚举（`TranslateColorGlyphRun`
→ 逐层 `CreateGlyphRunAnalysis` → `GetAlphaTextureBounds`）把各彩色层包围盒
**并**起来还回去。两边唯一的差别是 `rasterize_color` 为把位图原点对到包围盒
左上角加了 `-origin` 反偏移，而包围盒对整体平移不变 —— 所以用零偏移算出的
并集拿去当 `glyph_bounds`，`rasterize_color` 反偏移后各层恰好落回
`[0, size]`，与位图严丝合缝，定位零误差。字体没有彩色表示时返回 `None`，
调用点维持零包围盒的原行为，非 emoji 字形完全不受影响。

补上这个之后，纯 COLR 字体就能打了：国旗、全部彩色 emoji 走 COLR v0 路径，
应用侧打包 Twemoji Mozilla（`assets/fonts/`，`assets::FLAG_EMOJI_FONT`），
Windows/Linux 上显式指名，macOS 继续用系统 Apple Color Emoji。

## 补丁三：窗口隐藏（`SW_HIDE`）

`vendor/gpui-pre` 的第二处补丁给 `PlatformWindow` 加了 `hide_window` /
`show_window`，Windows 这一半就是 `ShowWindowAsync`：

- **隐藏**：`SW_HIDE`。它同时把任务栏按钮一并拿掉 —— 这正是它和 `SW_MINIMIZE`
  的区别，也是托盘"关窗缩起来"要的那一步。`SW_HIDE` 会让系统发
  `WM_SHOWWINDOW`，`events.rs` 那边照常把它翻译成 GPUI 的可见性变化，应用侧
  `Window::visibility()` 与 `observe_window_visibility` 都会跟着翻面。
- **显示**：已经可见就返回（幂等）。不可见时看 `IsIconic`：先被最小化过再隐藏的
  窗口，直接 `SW_SHOW` 回来仍是图标态，所以那一支走 `SW_RESTORE` 把两件事一起
  撤掉，其余走 `SW_SHOW`。

`Platform::supports_window_hiding` 在这边恒为 `true`。

顺带一提，`SW_HIDE` 这条路本来就在这份 crate 里 —— `WindowsWindow` 的 `Drop` 就用它把窗口
从屏幕上撤掉再 `DestroyWindow`。补丁只是把那句话拆出来，交给应用按需触发，而不
再只发生在窗口已经要销毁的那条路上。

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `src/cursor_image.rs` | 新文件：`CursorImageCache`（按图 id 缓存、按引用回收）、`NativeCursor`、`create_cursor`，以及单测（三种形式各自做出来的 `ICONINFO`、`DrawIconEx` 画到白底上实际得到的像素、掩码按 WORD 对齐的打包、窗口还在用时不销毁）。 |
| `src/gpui_windows.rs` | `mod cursor_image` 与 `pub(crate) use`。 |
| `src/platform.rs` | `WindowsPlatformState` 多一个 `cursor_images`；原 `set_cursor_style` 的函数体挪进 `apply_cursor`（外加回收），`set_cursor_style` 调它；实现 `set_cursor_image` 与 `supports_cursor_images`。补丁三：实现 `supports_window_hiding`。 |
| `src/events.rs` | `handle_set_cursor` 记下这次是不是本窗口定的光标；`handle_cursor_changed` 在上面说的条件下立刻 `SetCursor`。 |
| `src/window.rs` | `WindowsWindowState` 多一个 `cursor_under_pointer`。补丁三：实现 `hide_window` / `show_window`。 |
| `src/direct_write.rs` | 补丁二：`raster_bounds` 空矩形判定收紧为 `<=`；空包围盒且 `params.is_emoji` 时调新加的 `color_glyph_run_bounds`（COLR 各层 alpha 包围盒的并集）兜底；非 emoji 与无彩色字形行为不变。另加全管线单测 `pure_colr_font_flag_rasterizes_to_visible_pixels`（整形 → 判定 → 包围盒 → 非全透明像素，字体字节引用应用侧 `assets/fonts/Twemoji.Mozilla.ttf`）。 |

除上述文件外，其余与 crates.io 上的 0.3.8 逐字节相同。单测跑法：

```sh
cargo test -p gpui-pre-windows --lib cursor_image
```

升级与改回上游的做法见 `vendor/gpui-pre/PATCHES.md`，两份一起动。
