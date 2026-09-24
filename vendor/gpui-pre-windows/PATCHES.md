# 这份 `gpui-pre-windows` 与上游的差别

`vendor/gpui-pre-windows` 逐字来自 crates.io 的 **gpui-pre-windows 0.3.6**
（Zed 的 `gpui_windows` 在 zed@bcf6582 的快照，Apache-2.0，`LICENSE-APACHE` 与
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

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `src/cursor_image.rs` | 新文件：`CursorImageCache`（按图 id 缓存、按引用回收）、`NativeCursor`、`create_cursor`，以及单测（三种形式各自做出来的 `ICONINFO`、掩码按 WORD 对齐的打包、窗口还在用时不销毁）。 |
| `src/gpui_windows.rs` | `mod cursor_image` 与 `pub(crate) use`。 |
| `src/platform.rs` | `WindowsPlatformState` 多一个 `cursor_images`；原 `set_cursor_style` 的函数体挪进 `apply_cursor`（外加回收），`set_cursor_style` 调它；实现 `set_cursor_image` 与 `supports_cursor_images`。 |
| `src/events.rs` | `handle_set_cursor` 记下这次是不是本窗口定的光标；`handle_cursor_changed` 在上面说的条件下立刻 `SetCursor`。 |
| `src/window.rs` | `WindowsWindowState` 多一个 `cursor_under_pointer`。 |

其余文件与 crates.io 上的 0.3.6 逐字节相同。单测跑法：

```sh
cargo test -p gpui-pre-windows --lib cursor_image
```

升级与改回上游的做法见 `vendor/gpui-pre/PATCHES.md`，两份一起动。
