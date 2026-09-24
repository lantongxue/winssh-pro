# 这份 `gpui-pre` 与上游的差别

`vendor/gpui-pre` 来自 crates.io 的 **gpui-pre 0.3.6**（Zed 的 `gpui` 在
zed@bcf6582 的快照，由 gpui-kit 发布，Apache-2.0，`LICENSE-APACHE` 与
`Cargo.toml.orig` 原样留着），`Cargo.toml` 里的 `[patch.crates-io]` 指着它。
版本号必须和 gpui-kit 钉的 `=0.3.6` 一致，patch 才会生效。

改动只有一件事：**让应用可以给一张光标位图，当真正的系统光标用**。
平台那一半在 `vendor/gpui-pre-windows`（见那边的 PATCHES.md）。

## 为什么要动它

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

## 接口

- `CursorImage`（新文件 `src/platform/cursor_image.rs`）：宽高、热点、像素。
  像素三种：`Hidden`（藏起光标）、`Bgra`（带直通 alpha）、`Masked`
  （AND 掩码 + XOR 颜色，经典的 Windows 光标语义，反色靠它）。
- `Window::set_cursor_image(&image, fallback, &hitbox)`：和 `set_cursor_style`
  一样在 paint 阶段按 hitbox 登记，优先级规则完全相同 —— 离开这块区域、被
  菜单或滚动条盖住时，GPUI 照常换成那边登记的光标。
- `Platform::set_cursor_image` / `Platform::supports_cursor_images`：带默认实现
  （不支持）。不支持的平台用 `fallback` 那个样式；应用可以问
  `App::supports_cursor_images` 自己备一个画进画面的替身。

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `src/platform/cursor_image.rs` | 新文件：`CursorImage`、`CursorPixels`、`CursorImageId`（进程内唯一，平台按它缓存原生光标）。 |
| `src/platform.rs` | `mod cursor_image` 与 `pub use`；`Platform` trait 加两个带默认实现的方法。 |
| `src/window.rs` | `CursorStyleRequest` 多一个 `image` 字段；新增 `Window::set_cursor_image`；`Frame::cursor_style` 改为返回整条请求（原来只返回样式）；`reset_cursor_style` 先试请求里的图，平台不接再用样式。`use crate::{…}` 多引一个 `CursorImage`（rustfmt 重排了那一段）。 |
| `src/app.rs` | 新增 `App::supports_cursor_images`。 |

另外**删掉了 `examples/`**（4.5 MB，主要是示例图片）。作为依赖时 Cargo 不编
示例，`Cargo.toml` 里那些 `[[example]]` 条目只在编示例时才会去找文件。其余文件
与 crates.io 上的 0.3.6 逐字节相同（`diff -r` 对照即可）。

## 测试放在哪

gpui-pre 自己的 lib 测试离开 zed 仓库编不过（`src/svg_renderer.rs` 在
`cfg(test)` 下 `include_bytes!` 了 zed 仓库里的字体），crates.io 上的原版也是
如此；删掉 `examples/` 之后，`cfg(test)` 下引用示例图片的那一处也找不到文件，
同样只影响这份测试。所以这份补丁的覆盖在别处：原生光标的生成与回收在 `vendor/gpui-pre-windows`
的单测里，`set_cursor_image` 在 paint 阶段的登记在应用的
`src/ui/elements/frame.rs`（`every_cursor_form_draws`）里。

## gpui-kit 升级 GPUI 时

gpui-kit 换一版 `gpui-pre` 快照后，这份补丁的版本号就对不上了：Cargo 会提示
patch 未使用、退回 crates.io 的原版，而应用用到的 `set_cursor_image` /
`CursorImage` 会直接编译报错 —— 不会静默丢功能。到时候：

1. 从 crates.io 取新版 `gpui-pre`（和 `gpui-pre-windows`）的源码，替换这两个目录；
2. 照上表把改动重打一遍（改动都带 `Local patch (see PATCHES.md)` 注释，
   在旧目录里 grep 一下就能列全），删掉 `examples/`；
3. 更新两份 PATCHES.md 里的版本号。

## 改回上游

哪天上游 GPUI 自己有了"应用光标位图"的接口，把应用侧（`src/ui/elements/frame.rs`、
`src/ui/panels/rdp.rs`）换到那个接口上，删掉本目录、`vendor/gpui-pre-windows`
与 `Cargo.toml` 里对应的两行 `[patch.crates-io]` 即可。
