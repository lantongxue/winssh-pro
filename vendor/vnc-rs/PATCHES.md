# 这份 `vnc-rs` 与上游的差别

`vendor/vnc-rs` 逐字来自 crates.io 的 **vnc-rs 0.6.0**（<https://github.com/HsuJv/vnc-rs>，
MIT OR Apache-2.0，两份 LICENSE 与 `.cargo_vcs_info.json` 都原样留着），
`Cargo.toml` 里的 `[patch.crates-io]` 指着它。最初的补丁是**扩展剪贴板**的消息编解码，加上经典正文改为按字节交给上层；
后续帧边界补丁见文末。

为什么要动它：RFB 的 `ClientCutText` / `ServerCutText` 正文按 RFC 6143 §7.5.6 /
§7.6.4 只是 Latin-1，中文装不进去；能装的是 RealVNC / TigerVNC / noVNC 共同的
**扩展剪贴板**（伪编码 `0xc0a1e5ce`，正文 UTF-8 加 zlib，长度取负当标记）。
上游 0.6.0 不支持这个扩展，且两条路都堵着：

- `VncEvent::Text(String)` 用 `from_utf8_lossy` 解正文 —— 对端按规范发来的
  Latin-1 单字节（`é` = `0xE9`）会被吃成 U+FFFD，信息没了；
- `X11Event::CopyText(String)` 恒发 `as_bytes()` 的 UTF-8，没有按字节控制的口子；
- 扩展消息的长度字段最高位置位，上游会当成 2 GiB 的经典正文，
  `length > MAX_TEXT` 直接 `Err` —— **会把整条会话带下水**。

0.6.0 已是 crates.io 上的最新版，没有"升级即修"的路，所以在这一份副本上打补丁。

## 逐文件改动

| 文件 | 改了什么 |
| --- | --- |
| `src/clipboard.rs` | **新增**。伪编码号、`FORMAT_*` / `ACTION_*` 常量、`Caps`、`ExtendedClipboard` 五个动作的读写（Provide 走 zlib），以及"读不懂/超限就丢掉、绝不把会话带下水"的边界处理。 |
| `src/config.rs` | `VncEncoding` 加 `ExtendedClipboardPseudo`（`0xc0a1e5ce`）；`from_wire` 里按常量比，不写重复的魔数。 |
| `src/event.rs` | `VncEvent::Text(Vec<u8>)`、`X11Event::CopyText(Vec<u8>)`（原来是 `String`）；新增 `VncEvent::Clipboard(ExtendedClipboard)` 与 `X11Event::Clipboard(ExtendedClipboard)`。 |
| `src/client/messages.rs` | `ClientMsg::ClientCutText(Vec<u8>)` 与 `ClientMsg::ExtendedClipboard(_)`；`ServerMsg::ServerCutText(Vec<u8>)` 与 `ServerMsg::ExtendedClipboard(_)`。读长度时看最高位分岔：置位走扩展，否则经典（超 `MAX_TEXT` 仍报错，与上游一致）。 |
| `src/client/connection.rs` | 事件到消息的两处转发；矩形编码的 match 补上 `ExtendedClipboardPseudo`（它是 `SetEncodings` 的伪编码，出现在 `FramebufferUpdate` 里就是协议错）。 |
| `src/client/connection/output.rs`、`src/client/connection/tests.rs`、`src/lib.rs` | 跟随上面的类型变化（测试与文档示例）；另外把"超限长度该被当场拒掉"那条用例分成经典与扩展两种字节布局，扩展那条用的是 2 GiB 的负数长度。 |

字符集策略**不在这份补丁里**：`CopyText` 上来什么字节就发什么字节，解码也只把
字节递上去。发 Latin-1 还是 UTF-8、扩展握手的时机与回退，都在应用侧
（`src/vnc/clipboard.rs`），那里才是"看对端脸色"的地方。

补丁里的新代码与注释都是中文，与 winssh-pro 其余代码一致；若将来想往上给
`vnc-rs` 提 PR，把中文注释翻成英文即可 —— 逻辑与上游风格是照着的
（`vnc-rs` 自己的 `codec/` 就在同一个文件里写清每个字节的来历）。

## 想改成 `refs/` 那样的子模块

`refs/` 下的 gpui-kit / alacritty / tty7 都是浅克隆的子模块，而这里放不下一个
子模块 —— 上游没有这个补丁，本仓库也没有对应的 fork 仓库。要挪过去：

1. 把这份目录推成一个仓库（自己 fork 一份 `HsuJv/vnc-rs`，把上面的改动提上去）；
2. 删掉 `vendor/vnc-rs`，`git submodule add <你的 fork> refs/vnc-rs`；
3. `Cargo.toml` 的 `[patch.crates-io]` 改指 `{ git = "…", rev = "…" }`。

但要注意 `AGENTS.md` 那条"禁止修改 `refs/` 下的代码"：换成子模块之后，这份
补丁就落在 `refs/` 里了，和 gpui-kit 那种"参考代码"混在一起。真要走那条路，
建议把规则改成"补丁只许进 `refs/vnc-rs`"这类点名例外。

## 改回上游（放弃中文剪贴板）

删掉 `Cargo.toml` 里的 `[patch.crates-io]` 段、`rm -rf vendor`，再把应用侧
`src/vnc/clipboard.rs` 换回 `String` 版的两个事件即可 —— 那时中文剪贴板退回
"经典路径上按 UTF-8 碰运气"，重音字母会退化成 U+FFFD。

## 完整帧边界

应用过去按定时器截取画布，但一个 RFB FramebufferUpdate 内可含多个矩形，
ZRLE/Tight 还会拆成多个事件。网络停在矩形之间时，定时器会把新旧像素混合
成一张画面。仅看事件队列暂时为空无法判断消息已经收完，应用侧没有可靠
的替代办法，所以在现有本地补丁上只补充协议已经知道的边界：

- `src/event.rs`：新增 `BeginFrame` / `EndFrame` 元数据事件。
- `src/client/connector.rs`：新增 `with_frame_events()`，只为显式订阅的消费者
  发送边界，不改变既有消费者的事件序列和容量为 2 的背压队列。
- `src/client/connection.rs`：在整条 FramebufferUpdate 前后发送边界；零矩形
  及 LastRect 正常闭合，坏矩形、截断正文不发送结束边界。
- `src/client/connection/tests.rs`：覆盖 LastRect、空更新与中断；原有尺寸
  测试加入边界后的事件位置相应调整。`queue_tests.rs` 传入默认不订阅参数。
  应用侧 TCP 集成测试另覆盖两矩形之间停顿 120 ms 不发布半帧。

不更改任何编解码器。应用在 EndFrame 才发布快照，刷新节拍只负责请求。
