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
//! 经典路径上**字符集是猜出来的**，因为那条正文里根本没有字符集字段：规范
//! 说 Latin-1，现实里各家服务器塞的是自己那台机器的本地编码 —— Linux /
//! macOS 一侧多半已是 UTF-8，Windows 一侧（TightVNC、UltraVNC、RealVNC 的
//! WinVNC）是系统 ANSI 代码页，简体中文那台就是 GBK。所以这一档做成了每台
//! 主机一格设置 [`VncClipboardCharset`]，两头都照它办：
//!
//! - **发**（[`classic_bytes`]）：点名了就按点名的那张表编（装不下的字换
//!   成 `?` —— TigerVNC 与 RealVNC 自己的客户端也是这么做的，规范里没有更好
//!   的答案）。留在自动档就是 Latin-1 装得下发单字节、装不下发 UTF-8，
//!   外加一条：这条会话上要是从对端正文里嗅出过传统编码（见下），之后就按
//!   嗅出来的那一种发。
//! - **收**（[`classic_text`]）：点名了就按那张表解。自动档先试 UTF-8，
//!   解不动再**严格**试一次 GBK（不许有一个字掉进替换字符，且解出来真有
//!   汉字），还不行才按规范当 Latin-1。上游 `vnc-rs` 在这里用的是
//!   `from_utf8_lossy`，一份 Latin-1 的 `é`（单字节 `0xE9`）会被吃成
//!   U+FFFD —— 重音字母的乱码出在那一步，中文的乱码则出在"根本没试过 GBK"。
//!
//! 自动档只嗅 GBK 一种：Big5 / Shift_JIS 的字节在 GBK 表里多半也"解得通"，
//! 只是解成另一堆汉字，嗅探分不开它们 —— 那两种（以及任何嗅错的场合）得在
//! 主机表单里点名，点名的那一档是确定的，不再嗅。
//!
//! 两个方向都顺手把行尾折成 LF：规范里换行只认 `\n`（"No carriage-return is
//! needed"），而 Windows 剪贴板给的是 CRLF；TigerVNC 的客户端收到正文里的
//! `\r` 甚至会直接抛异常。

use std::borrow::Cow;

use encoding_rs::Encoding;
use vnc::clipboard::{
    ACTION_CAPS, ACTION_NOTIFY, ACTION_PEEK, ACTION_PROVIDE, ACTION_REQUEST, FORMAT_TEXT,
};
use vnc::{Caps, ExtendedClipboard, X11Event};

use super::settings::VncClipboardCharset;

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

/// 一条会话上剪贴板的状态：对端能力、本地此刻的文本、Caps 回过没有，
/// 加上经典路径的字符集这本账。
///
/// 每个字段都是"这条会话"的，断开重连就该换一份新的（本地文本的基线由面板
/// 在连上那一刻重设，见 [`VncPanel`](crate::ui::panels::VncPanel)）。
#[derive(Debug, Default)]
pub(crate) struct ClipboardSync {
    /// 对端报出来的能力。`None` = 还没收到 Caps，经典路径。
    remote: Option<Caps>,
    /// 本地此刻的文本：扩展路径下对端 Request 时要现取现给。
    local: Option<String>,
    /// 我们那份 Caps 回过对端没有。回一次就够，且只在对端先递 Caps 之后回。
    announced: bool,
    /// 这台主机点的经典正文字符集（[`VncClipboardCharset::Auto`] = 没点名）。
    charset: VncClipboardCharset,
    /// 自动档下从对端正文里嗅出来的传统编码。嗅出来之后**发**侧也按它发 ——
    /// 没有这一条，对端是中文 Windows 时"远端 → 本地"能通而"本地 → 远端"
    /// 仍旧乱码，两个方向不对称。点过名的会话不会走到这里。
    sniffed: Option<VncClipboardCharset>,
}

impl ClipboardSync {
    /// 按这台主机点的字符集起一份。
    pub(crate) fn new(charset: VncClipboardCharset) -> Self {
        Self {
            charset,
            ..Self::default()
        }
    }

    /// 经典正文此刻按哪一档发：点过名就是点的那一档，没点名就看嗅出来的，
    /// 都没有才是自动。
    fn outgoing_charset(&self) -> VncClipboardCharset {
        match self.charset {
            VncClipboardCharset::Auto => self.sniffed.unwrap_or(VncClipboardCharset::Auto),
            named => named,
        }
    }

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
                    log::warn!("VNC 剪贴板文本 {} 字节超过对端上限，这条不同步", text.len(),);
                    return None;
                }
                X11Event::Clipboard(ExtendedClipboard::Notify {
                    formats: FORMAT_TEXT,
                })
            }
            _ => {
                let charset = self.outgoing_charset();
                log::debug!(
                    "VNC 剪贴板：对端没有扩展剪贴板，{} 字节按 {charset:?} 走经典正文",
                    text.len(),
                );
                X11Event::CopyText(classic_bytes(&text, charset))
            }
        };
        self.local = Some(text);
        Some(event)
    }

    /// 对端来了一条经典正文 → 解成文本。
    ///
    /// 解出来的就是本地剪贴板接下来的内容（前台会写进去），所以顺手记成
    /// "我们手上这份" —— 对端随后要是来 Peek / Request，得给它这一份。
    ///
    /// 自动档下顺手记住"这份正文是按哪种传统编码解通的"：往后发侧按同一种
    /// 编，两个方向才对称（见 [`Self::sniffed`]）。
    pub(crate) fn on_classic(&mut self, bytes: &[u8]) -> String {
        let decoded = classic_text(bytes, self.charset);
        if let Some(sniffed) = decoded.sniffed
            && self.sniffed != Some(sniffed)
        {
            log::info!("VNC 剪贴板：对端的经典正文按 {sniffed:?} 解得通，这条会话往后也按它发");
            self.sniffed = Some(sniffed);
        }
        self.local = Some(decoded.text.clone());
        decoded.text
    }

    /// 对端来了一条扩展消息 → 该回什么、该不该写本地剪贴板。
    pub(crate) fn on_extended(&mut self, message: ExtendedClipboard) -> Reaction {
        match message {
            ExtendedClipboard::Caps(caps) => {
                // 对端先递了 Caps，说明它认这个扩展：回敬一份我们的能力。
                // 只报一次 —— 有些实现对同一条会话会重复递 Caps。
                log::info!(
                    "VNC 剪贴板：对端认扩展剪贴板（formats={:#x} actions={:#x}），中文走 UTF-8 那条路，主机上点的字符集不参与",
                    caps.formats,
                    caps.actions,
                );
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
                reply: (formats & FORMAT_TEXT != 0 && self.announced && self.local.is_some()).then(
                    || {
                        X11Event::Clipboard(ExtendedClipboard::Notify {
                            formats: FORMAT_TEXT,
                        })
                    },
                ),
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

/// 经典路径的正文：文本 → 按 `charset` 编出来的字节。
///
/// 自动档是"装得下就 Latin-1 单字节，装不下就 UTF-8"——"装得下"看的是码点
/// ≤ U+00FF，ASCII 段两种编码的字节本来相同，所以只有 U+0080–U+00FF 这一段
/// （重音字母一类）会因这个选择而不同；中文装不下，发 UTF-8 是"对端认
/// UTF-8 就赚到"（x11vnc、libvncserver 这类字节直通的实现，macOS 的屏幕共享
/// 也是）。点了名就按点的那张表编，编不下的字换成 `?`。
pub(crate) fn classic_bytes(text: &str, charset: VncClipboardCharset) -> Vec<u8> {
    let text = convert_lf(text);
    match charset {
        VncClipboardCharset::Auto => {
            if text.chars().all(|c| c as u32 <= 0xff) {
                text.chars().map(|c| c as u8).collect()
            } else {
                text.into_owned().into_bytes()
            }
        }
        VncClipboardCharset::Latin1 => text
            .chars()
            .map(|c| if c as u32 <= 0xff { c as u8 } else { b'?' })
            .collect(),
        named => encode_with(
            named
                .table()
                .expect("自动与 Latin-1 之外的每一档都有一张表"),
            &text,
        ),
    }
}

/// 一份文本按某张表编成字节。编不下的字换成 `?` ——`encoding_rs` 的
/// `encode` 对编不下的字给的是 HTML 数字引用（`&#26085;`），那东西落进
/// 剪贴板只会比一个问号更费解。逐字重编只在真有编不下的字时走。
fn encode_with(table: &'static Encoding, text: &str) -> Vec<u8> {
    let (bytes, _, unmappable) = table.encode(text);
    if !unmappable {
        return bytes.into_owned();
    }
    log::warn!(
        "VNC 剪贴板：有字在 {} 里没有位置，换成问号发出去",
        table.name()
    );
    let mut out = Vec::with_capacity(text.len());
    let mut buffer = [0u8; 4];
    for character in text.chars() {
        let (chunk, _, unmappable) = table.encode(character.encode_utf8(&mut buffer));
        if unmappable {
            out.push(b'?');
        } else {
            out.extend_from_slice(&chunk);
        }
    }
    out
}

/// 一条经典正文解出来的东西：文本，以及"这是嗅出来的哪一种传统编码"
/// （点过名的档、以及 UTF-8 / Latin-1 这两种不用记的，都是 `None`）。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Decoded {
    pub(crate) text: String,
    pub(crate) sniffed: Option<VncClipboardCharset>,
}

/// 经典路径的正文 → 文本，按 `charset` 解。
///
/// 自动档是三步：UTF-8 解得动就是 UTF-8；解不动**严格**试一次 GBK（见
/// [`sniff_gbk`]）；都不行才按规范当 Latin-1（每个字节一个码点）。点了名
/// 就只按那张表解，解不动的字节由 `encoding_rs` 换成替换字符 —— 点名的意思
/// 就是"别再猜了"。
pub(crate) fn classic_text(bytes: &[u8], charset: VncClipboardCharset) -> Decoded {
    // 有些服务器会补一个 NUL 结尾（TigerVNC 的解码就停在第一个 NUL）。
    let bytes = match bytes.iter().position(|byte| *byte == 0) {
        Some(end) => &bytes[..end],
        None => bytes,
    };
    match charset {
        VncClipboardCharset::Auto => {
            if let Ok(text) = std::str::from_utf8(bytes) {
                return Decoded {
                    text: text.to_string(),
                    sniffed: None,
                };
            }
            if let Some(text) = sniff_gbk(bytes) {
                return Decoded {
                    text,
                    sniffed: Some(VncClipboardCharset::Gbk),
                };
            }
            Decoded {
                text: latin1(bytes),
                sniffed: None,
            }
        }
        VncClipboardCharset::Latin1 => Decoded {
            text: latin1(bytes),
            sniffed: None,
        },
        named => Decoded {
            text: named
                .table()
                .expect("自动与 Latin-1 之外的每一档都有一张表")
                .decode_without_bom_handling(bytes)
                .0
                .into_owned(),
            sniffed: None,
        },
    }
}

/// 按规范把每个字节当一个码点（Latin-1）。
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

/// 这串字节是不是一份 GBK 的中文？三道关一起把西文挡在外面：
///
/// 1. **每个高字节都得和另一个高字节成对**（[`double_byte_pairs`]）。一份
///    Latin-1 的西文里高字节多半是孤立的（`naïve` 的 `ï` 两边都是 ASCII），
///    而常用汉字在 GBK 里两个字节都 ≥ 0x80。这一关挡掉的正是"高字节后面
///    跟着一个 ASCII 字母"那种被 GBK 表当成生僻字吃掉的情况。
/// 2. **严格解**：只要有一个字节在 GBK 里没有位置就不算。重音字母连着出现
///    时的字节对（`Grüße` 的 `üß` = `FC DF`）恰好落在 GBK 的用户自定义区，
///    解不出来，于是交还给 Latin-1。
/// 3. 解出来得**真有汉字**（或假名、谚文、全角标点），不然一份普通西文也
///    会"解得通"。
///
/// 三道关之后仍有解不开的歧义 —— `ÄÖ`（`C4 D6`）在 GBK 里是一个正经汉字，
/// 两种读法都成立。这类文本得在主机表单里把字符集点成 Latin-1；嗅探本来
/// 就只是"没点名时的默认答案"。
fn sniff_gbk(bytes: &[u8]) -> Option<String> {
    if !double_byte_pairs(bytes) {
        return None;
    }
    let text = encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(bytes)?;
    looks_cjk(&text).then(|| text.into_owned())
}

/// 高字节是不是都两两成对。ASCII 单走，≥ 0x80 的字节必须和紧跟着的另一个
/// ≥ 0x80 的字节凑成一对。
fn double_byte_pairs(bytes: &[u8]) -> bool {
    let mut index = 0;
    while let Some(byte) = bytes.get(index) {
        if *byte < 0x80 {
            index += 1;
            continue;
        }
        match bytes.get(index + 1) {
            Some(next) if *next >= 0x80 => index += 2,
            _ => return false,
        }
    }
    true
}

/// 解出来的东西里有没有汉字、假名、谚文或全角标点。
fn looks_cjk(text: &str) -> bool {
    text.chars().any(|character| {
        matches!(character as u32,
            0x2e80..=0x9fff   // 部首扩展 → CJK 标点 → 假名 → 谚文 → 汉字
            | 0xac00..=0xd7af // 谚文音节
            | 0xf900..=0xfaff // 兼容汉字
            | 0xff00..=0xffef // 全角形式
        )
    })
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
#[path = "tests/vnc/clipboard.rs"]
mod tests;
