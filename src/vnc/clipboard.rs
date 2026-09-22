//! 剪贴板那条线的规矩：本地文本 ↔ 线上的两条剪贴板路径。
//!
//! RFB 的剪贴板消息只有一条 —— `ClientCutText` / `ServerCutText` —— 但正文明
//! 两种走法：
//!
//! - **经典**：正文按 RFC 6143 §7.5.6 / §7.6.4 是 Latin-1（每个字节一个码点，
//!   U+00FF 封顶），中文根本装不进去。
//! - **扩展**（`ExtendedClipboard`）：RealVNC / TigerVNC / noVNC 的共同扩展，
//!   正文是 UTF-8 加 zlib，长度字段取负当标记。要先握手：客户端在
//!   `SetEncodings` 里报出伪编码 `0xc0a1e5ce`，对端支持就先递一份 Caps
//!   （它认哪些格式、哪些动作、每个格式愿意收多大），我们回敬一份 Caps，
//!   往后就是 Notify → Request → Provide 三步走。
//!
//! 走哪条**全看对端**：收到对面的 Caps 之前一律经典。对不认识这个扩展的
//! 服务器，一条最高位置位的长度是天文数字，它只会当成一条超大的 Latin-1 正文
//! ——所以我们那边也只在**对端先递过 Caps** 之后才发扩展消息（见 [`ClipboardSync`]）。
//!
//! 经典路径的字符集是"规范说 Latin-1，现实里两种都有"，两头都照现实办：
//!
//! - **发**（[`classic_bytes`]）：Latin-1 装得下的文本就发 Latin-1 单字节 ——
//!   对按规范读的对端（TigerVNC、RealVNC）是对的，对按 UTF-8 读的也对，两种
//!   读法在这段字节上恰好一致；装不下的（中文、emoji）发 UTF-8。TigerVNC 与
//!   RealVNC 自己的客户端在装不下时把字符换成 `?`，因为规范里没有正确答案；
//!   我们发 UTF-8 是"对端认 UTF-8 就赚到"（x11vnc、libvncserver 这类字节直通
//!   的实现，macOS 的屏幕共享也是），遇上按 Latin-1 读的对端只是乱码一份 ——
//!   那种对端的中文得靠扩展剪贴板，经典路径救不回来。
//! - **收**（[`classic_text`]）：能按 UTF-8 解就 UTF-8，解不动就按规范当
//!   Latin-1。上游 `vnc-rs` 在这里用的是 `from_utf8_lossy`，一份 Latin-1 的
//!   `é`（单字节 `0xE9`）会被吃掉成 U+FFFD —— 重音字母与一部分中文乱码就出在
//!   这一步，我们把它换成了这条两步判断。
//!
//! 两个方向都顺手把行尾折成 LF：规范里换行只认 `\n`（"No carriage-return is
//! needed"），而 Windows 剪贴板给的是 CRLF；TigerVNC 的客户端收到正文里的
//! `\r` 甚至会直接抛异常。

use std::borrow::Cow;

use vnc::clipboard::{
    ACTION_CAPS, ACTION_NOTIFY, ACTION_PEEK, ACTION_PROVIDE, ACTION_REQUEST, FORMAT_TEXT,
};
use vnc::{Caps, ExtendedClipboard, X11Event};

/// 剪贴板文本的字节上限，与 `vnc-rs` 自己 `limits::MAX_TEXT` 的数对齐。
/// 超了它 `vnc-rs` 会在读侧直接掐断连接 —— 一屏复制误抄进一整本日志时，
/// 宁可这条剪贴板同步被跳过，也不能把会话带下水。扩展路径上它还是我们
/// 报给对端的接收上限（Caps 里那个数）。
pub(crate) const MAX_CUT_TEXT: usize = 1024 * 1024;

/// 本地复制了东西该发什么、对端来了一条该回什么。
#[derive(Debug, Default)]
pub(crate) struct Reaction {
    /// 要回给对端的消息（没有就不发）。
    pub(crate) reply: Option<X11Event>,
    /// 要写进系统剪贴板的文本（没有就不写）。
    pub(crate) clipboard: Option<String>,
}

/// 一条会话上剪贴板的状态：对端能力、本地此刻的文本、Caps 回过没有。
///
/// 三个字段都是"这条会话"的，断开重连就该换一份新的（本地文本的基线由面板
/// 在连上那一刻重设，见 [`VncPanel`](crate::ui::panels::VncPanel)）。
#[derive(Debug, Default)]
pub(crate) struct ClipboardSync {
    /// 对端报出来的能力。`None` = 还没收到 Caps，经典路径。
    remote: Option<Caps>,
    /// 本地此刻的文本：扩展路径下对端 Request 时要现取现给。
    local: Option<String>,
    /// 我们那份 Caps 回过对端没有。回一次就够，且只在对端先递 Caps 之后回。
    announced: bool,
}

impl ClipboardSync {
    /// 本地复制了新东西 → 给出发给对端的那条消息（没得发就是 `None`）。
    ///
    /// 对端认扩展且收文本，就只报一声"我有了"（`Notify`），正文等它来要 ——
    /// 中文因此从不过经典那条 Latin-1 的路。否则老老实实把正文推过去。
    ///
    /// 对端 Caps 里报过接收上限、这份文本又超了，就整条跳过（`local` 也不动）：
    /// 报了却拿不出东西比不报更让对端困惑，而悄悄换一份小一号的文本更坏。
    pub(crate) fn local_text(&mut self, text: String) -> Option<X11Event> {
        let event = match &self.remote {
            Some(caps) if speaks_extended(caps) => {
                if !within_limit(caps, text.len()) {
                    log::warn!(
                        "VNC 剪贴板文本 {} 字节超过对端上限，这条不同步",
                        text.len(),
                    );
                    return None;
                }
                X11Event::Clipboard(ExtendedClipboard::Notify {
                    formats: FORMAT_TEXT,
                })
            }
            _ => X11Event::CopyText(classic_bytes(&text)),
        };
        self.local = Some(text);
        Some(event)
    }

    /// 对端来了一条经典正文 → 解成文本。
    ///
    /// 解出来的就是本地剪贴板接下来的内容（前台会写进去），所以顺手记成
    /// "我们手上这份" —— 对端随后要是来 Peek / Request，得给它这一份。
    pub(crate) fn on_classic(&mut self, bytes: &[u8]) -> String {
        let text = classic_text(bytes);
        self.local = Some(text.clone());
        text
    }

    /// 对端来了一条扩展消息 → 该回什么、该不该写本地剪贴板。
    pub(crate) fn on_extended(&mut self, message: ExtendedClipboard) -> Reaction {
        match message {
            ExtendedClipboard::Caps(caps) => {
                // 对端先递了 Caps，说明它认这个扩展：回敬一份我们的能力。
                // 只报一次 —— 有些实现对同一条会话会重复递 Caps。
                let reply = (!self.announced).then(|| {
                    self.announced = true;
                    X11Event::Clipboard(ExtendedClipboard::Caps(own_caps()))
                });
                self.remote = Some(caps);
                Reaction {
                    reply,
                    clipboard: None,
                }
            }
            ExtendedClipboard::Request { formats } => Reaction {
                reply: self.provide(formats),
                clipboard: None,
            },
            ExtendedClipboard::Peek { formats } => Reaction {
                // "你手上有多大"：报一声"有"就够了，数据等它来 Request。
                reply: (formats & FORMAT_TEXT != 0
                    && self.announced
                    && self.local.is_some())
                .then(|| {
                    X11Event::Clipboard(ExtendedClipboard::Notify {
                        formats: FORMAT_TEXT,
                    })
                }),
                clipboard: None,
            },
            ExtendedClipboard::Notify { formats } => Reaction {
                // 对端说它有新文本：去要。只有它 Caps 里报了会应答 Request
                // 才去要 —— 否则这一声只会掉在地上。
                reply: (formats & FORMAT_TEXT != 0 && self.supports(ACTION_REQUEST)).then(|| {
                    X11Event::Clipboard(ExtendedClipboard::Request {
                        formats: FORMAT_TEXT,
                    })
                }),
                clipboard: None,
            },
            ExtendedClipboard::Provide { items } => {
                let text = items
                    .iter()
                    .find(|(format, _)| *format == FORMAT_TEXT)
                    .map(|(_, data)| extended_text(data));
                // 这份文本接下来就是本地剪贴板的内容（前台会写进去），
                // 记成"我们手上这份"：对端随后再来 Peek / Request 得给这一份。
                if let Some(text) = &text {
                    self.local = Some(text.clone());
                }
                Reaction {
                    reply: None,
                    clipboard: text,
                }
            }
            // 读不懂的一条：丢掉，会话照常。
            ExtendedClipboard::Ignored => Reaction::default(),
        }
    }

    /// 对端来 Request：把本地那份文本 Provide 出去。
    fn provide(&self, formats: u32) -> Option<X11Event> {
        if formats & FORMAT_TEXT == 0 || !self.announced {
            return None;
        }
        let text = self.local.as_deref()?;
        let mut bytes = convert_lf(text).into_owned().into_bytes();
        // 上限在 [`Self::local_text`] 那次已经看过一眼，这里再看一次是防
        // "对端回过一份新 Caps，把上限改小了"。超了就不发：一份被对端
        // 悄悄丢掉的文本，比"没同步"更难查。
        if let Some(caps) = &self.remote
            && !within_limit(caps, bytes.len())
        {
            log::warn!(
                "VNC 剪贴板文本 {} 字节超过对端上限，这条不同步",
                bytes.len(),
            );
            return None;
        }
        // 文本块按惯例以 NUL 收尾（规范里的文本格式这么写，noVNC 也这么发）。
        // 按长度读的实现（TigerVNC）多这一个字节无所谓，按 C 字符串读的
        // 实现少了它就会读飞。
        bytes.push(0);
        Some(X11Event::Clipboard(ExtendedClipboard::Provide {
            items: vec![(FORMAT_TEXT, bytes)],
        }))
    }

    /// 对端 Caps 里认不认这个动作。
    fn supports(&self, action: u32) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|caps| caps.actions & action != 0)
    }
}

/// 对端认不认"扩展剪贴板 + 文本 + 收得到 Provide"这一套。
fn speaks_extended(caps: &Caps) -> bool {
    caps.formats & FORMAT_TEXT != 0
        && caps.actions & ACTION_NOTIFY != 0
        && caps.actions & ACTION_PROVIDE != 0
}

/// 这份文本在对端报的接收上限之内吗。上限没报过（或报 0）就当不限制。
fn within_limit(caps: &Caps, bytes: usize) -> bool {
    caps.limit(FORMAT_TEXT)
        .filter(|limit| *limit > 0)
        .is_none_or(|limit| bytes <= limit as usize)
}

/// 我们自己那份 Caps：只报文本一个格式，动作报全套（`Caps` 位本身也在里面，
/// 对端就靠那一位认出这是条 Caps），并声明能收多大。
fn own_caps() -> Caps {
    Caps {
        formats: FORMAT_TEXT,
        actions: ACTION_CAPS | ACTION_REQUEST | ACTION_PEEK | ACTION_NOTIFY | ACTION_PROVIDE,
        limits: vec![(FORMAT_TEXT, MAX_CUT_TEXT as u32)],
    }
}

/// 经典路径的正文：装得下就 Latin-1 单字节，装不下就 UTF-8。
///
/// "装得下"看的是码点 ≤ U+00FF。ASCII 段两种编码的字节相同，所以只有
/// U+0080–U+00FF 这一段（重音字母一类）会因这个选择而不同。
pub(crate) fn classic_bytes(text: &str) -> Vec<u8> {
    let text = convert_lf(text);
    if text.chars().all(|c| c as u32 <= 0xff) {
        text.chars().map(|c| c as u8).collect()
    } else {
        text.into_owned().into_bytes()
    }
}

/// 经典路径的正文 → 文本：UTF-8 解得动就是 UTF-8，解不动按规范当 Latin-1
/// （每个字节一个码点）。两种对端都各得其所。
pub(crate) fn classic_text(bytes: &[u8]) -> String {
    // 有些服务器会补一个 NUL 结尾（TigerVNC 的解码就停在第一个 NUL）。
    let bytes = match bytes.iter().position(|byte| *byte == 0) {
        Some(end) => &bytes[..end],
        None => bytes,
    };
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|byte| char::from(*byte)).collect(),
    }
}

/// 扩展路径的正文 → 文本：规范里这一条就是 UTF-8（末尾可能有 NUL）。
fn extended_text(bytes: &[u8]) -> String {
    let bytes = match bytes.iter().position(|byte| *byte == 0) {
        Some(end) => &bytes[..end],
        None => bytes,
    };
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(error) => {
            log::warn!("VNC 扩展剪贴板正文不是合法 UTF-8：{error}，按替换字符收下");
            String::from_utf8_lossy(bytes).into_owned()
        }
    }
}

/// 行尾折成 LF：CRLF 与单个 CR 都变成 LF。规范里剪贴板正文只认 `\n`。
fn convert_lf(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(current) = chars.next() {
        if current == '\r' {
            // CRLF 只算一次换行；后面那个 LF 吃掉。
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(current);
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vnc::clipboard::FORMAT_HTML;

    fn caps(formats: u32, actions: u32) -> Caps {
        Caps {
            formats,
            actions,
            limits: vec![(FORMAT_TEXT, 1 << 20)],
        }
    }

    fn extended() -> ClipboardSync {
        let mut sync = ClipboardSync::default();
        let reaction = sync.on_extended(ExtendedClipboard::Caps(caps(
            FORMAT_TEXT,
            ACTION_CAPS | ACTION_REQUEST | ACTION_NOTIFY | ACTION_PROVIDE,
        )));
        assert!(
            matches!(reaction.reply, Some(X11Event::Clipboard(ExtendedClipboard::Caps(_)))),
            "对端递 Caps 该回敬一份 Caps",
        );
        sync
    }

    #[test]
    fn classic_sends_latin1_when_it_fits_and_utf8_when_it_does_not() {
        assert_eq!(classic_bytes("git status"), b"git status");
        // é 是单字节 Latin-1：按规范读的对端与按 UTF-8 读的对端都能认出来。
        assert_eq!(classic_bytes("café"), vec![0x63, 0x61, 0x66, 0xE9]);
        // 中文装不进 Latin-1，只能按 UTF-8 发。
        assert_eq!(classic_bytes("你好"), "你好".as_bytes());
    }

    #[test]
    fn classic_line_endings_become_lf() {
        assert_eq!(classic_bytes("a\r\nb\rc"), b"a\nb\nc");
        assert_eq!(classic_bytes("a\nb"), b"a\nb");
    }

    #[test]
    fn classic_receives_utf8_or_latin1() {
        assert_eq!(classic_text("你好".as_bytes()), "你好");
        assert_eq!(classic_text(&[0x63, 0x61, 0x66, 0xE9]), "café");
        // 末尾的 NUL 不算文本。
        assert_eq!(classic_text(b"hi\0"), "hi");
    }

    /// 一条消息是不是"经典正文，内容等于 `expected`"。
    fn is_classic(event: Option<X11Event>, expected: &[u8]) -> bool {
        matches!(event, Some(X11Event::CopyText(bytes)) if bytes == expected)
    }

    #[test]
    fn without_caps_the_classic_path_carries_the_text() {
        let mut sync = ClipboardSync::default();
        let event = sync.local_text("你好".into());
        assert!(
            is_classic(event, "你好".as_bytes()),
            "对端没报 Caps，正文该按经典路径出去",
        );
    }

    #[test]
    fn with_caps_a_local_copy_only_notifies_and_provides_on_request() {
        let mut sync = extended();
        let event = sync.local_text("你好".into());
        assert!(
            matches!(
                event,
                Some(X11Event::Clipboard(ExtendedClipboard::Notify {
                    formats: FORMAT_TEXT
                }))
            ),
            "扩展路径下正文不该跟着 Notify 一起出门，实际 {event:?}",
        );

        let reaction = sync.on_extended(ExtendedClipboard::Request {
            formats: FORMAT_TEXT,
        });
        match reaction.reply {
            Some(X11Event::Clipboard(ExtendedClipboard::Provide { items })) => {
                // 文本块末尾那个 NUL 是惯例（见 [`ClipboardSync::provide`]）。
                assert_eq!(items, vec![(FORMAT_TEXT, "你好\0".as_bytes().to_vec())]);
            }
            other => panic!("对端来要，该 Provide 一份文本，实际 {other:?}"),
        }
    }

    #[test]
    fn a_notify_from_the_remote_is_answered_with_a_request() {
        let mut sync = extended();
        let reaction = sync.on_extended(ExtendedClipboard::Notify {
            formats: FORMAT_TEXT,
        });
        assert!(
            matches!(
                reaction.reply,
                Some(X11Event::Clipboard(ExtendedClipboard::Request { formats: FORMAT_TEXT }))
            ),
            "收到 Notify 该去要，实际 {:?}",
            reaction.reply,
        );
        assert!(reaction.clipboard.is_none());

        let reaction = sync.on_extended(ExtendedClipboard::Provide {
            items: vec![(FORMAT_TEXT, "远端复制的中文".as_bytes().to_vec())],
        });
        assert_eq!(reaction.clipboard.as_deref(), Some("远端复制的中文"));
    }

    #[test]
    fn a_peek_is_answered_with_a_notify_only_when_there_is_something_to_offer() {
        let mut sync = extended();
        let reaction = sync.on_extended(ExtendedClipboard::Peek {
            formats: FORMAT_TEXT,
        });
        assert!(reaction.reply.is_none(), "手上没有文本就别报有");

        sync.local_text("你好".into());
        let reaction = sync.on_extended(ExtendedClipboard::Peek {
            formats: FORMAT_TEXT,
        });
        assert!(
            matches!(
                reaction.reply,
                Some(X11Event::Clipboard(ExtendedClipboard::Notify { formats: FORMAT_TEXT }))
            ),
            "Peek 只是问有没有，回一声 Notify 就够，实际 {:?}",
            reaction.reply,
        );
    }

    #[test]
    fn the_remote_limit_is_honoured() {
        let mut sync = ClipboardSync::default();
        sync.on_extended(ExtendedClipboard::Caps(Caps {
            formats: FORMAT_TEXT,
            actions: ACTION_CAPS | ACTION_REQUEST | ACTION_NOTIFY | ACTION_PROVIDE,
            limits: vec![(FORMAT_TEXT, 4)],
        }));
        assert!(
            sync.local_text("五个字以上".into()).is_none(),
            "超过对端声明的上限，连 Notify 都不该发",
        );
        let reaction = sync.on_extended(ExtendedClipboard::Request {
            formats: FORMAT_TEXT,
        });
        assert!(reaction.reply.is_none(), "没报过、也没存下来，来要也没有");
    }

    #[test]
    fn a_peer_without_the_text_format_stays_on_the_classic_path() {
        let mut sync = ClipboardSync::default();
        // 认这个扩展，但只收 HTML —— 没有能放我们文本的地方。
        sync.on_extended(ExtendedClipboard::Caps(caps(
            FORMAT_HTML,
            ACTION_CAPS | ACTION_REQUEST | ACTION_NOTIFY | ACTION_PROVIDE,
        )));
        assert!(is_classic(sync.local_text("你好".into()), "你好".as_bytes()));
    }

    #[test]
    fn a_peer_that_cannot_be_asked_is_not_asked() {
        let mut sync = ClipboardSync::default();
        // 会通知我们，但 Caps 里没报 REQUEST：问了也没人应。
        sync.on_extended(ExtendedClipboard::Caps(caps(
            FORMAT_TEXT,
            ACTION_CAPS | ACTION_NOTIFY | ACTION_PROVIDE,
        )));
        let reaction = sync.on_extended(ExtendedClipboard::Notify {
            formats: FORMAT_TEXT,
        });
        assert!(reaction.reply.is_none(), "对端不认 Request，就别去要");
    }

    #[test]
    fn an_inbound_text_becomes_what_we_offer_back() {
        let mut sync = extended();
        // 对端复制来的文本会写进本地剪贴板 —— 那我们手上这份就该换成它，
        // 否则对端随后再来问，我们给的是上一份、早就不是剪贴板里的内容了。
        assert_eq!(sync.on_classic("经典来的".as_bytes()), "经典来的");
        let reaction = sync.on_extended(ExtendedClipboard::Peek {
            formats: FORMAT_TEXT,
        });
        assert!(
            matches!(
                reaction.reply,
                Some(X11Event::Clipboard(ExtendedClipboard::Notify { .. }))
            ),
            "剪贴板里已经是这份文本，对端来问就该说有",
        );

        sync.on_extended(ExtendedClipboard::Provide {
            items: vec![(FORMAT_TEXT, "扩展来的".as_bytes().to_vec())],
        });
        let reaction = sync.on_extended(ExtendedClipboard::Request {
            formats: FORMAT_TEXT,
        });
        assert!(
            matches!(
                &reaction.reply,
                Some(X11Event::Clipboard(ExtendedClipboard::Provide { items }))
                    if *items == vec![(FORMAT_TEXT, "扩展来的\0".as_bytes().to_vec())]
            ),
            "给出去的得是手上这一份（对端刚给我们的那份），实际 {:?}",
            reaction.reply,
        );
    }

    #[test]
    fn the_own_caps_reply_goes_out_once() {
        let mut sync = extended();
        let again = sync.on_extended(ExtendedClipboard::Caps(caps(
            FORMAT_TEXT,
            ACTION_CAPS | ACTION_NOTIFY | ACTION_PROVIDE,
        )));
        assert!(again.reply.is_none(), "Caps 只回一次");
    }
}
