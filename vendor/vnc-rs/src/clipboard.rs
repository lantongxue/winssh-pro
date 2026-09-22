//! Extended Clipboard —— RFB 的社区扩展，用来把规范放不下的文本送上线路。
//!
//! 经典 `ServerCutText` / `ClientCutText` 的正文按 RFC 6143 §7.6.4 / §7.5.6 只是
//! Latin-1（单字节，U+00FF 封顶）—— 中文、日文、emoji 一律进不去。RealVNC、
//! TigerVNC、noVNC 一起把这条消息扩成"扩展剪贴板"：客户端在 `SetEncodings` 里
//! 报出伪编码 [`ENCODING_EXTENDED_CLIPBOARD`]，正文换成 `flags: u32` 加各格式的
//! 数据块，文本按 UTF-8 走，长度字段取负值（最高位置 1）当作"这是扩展"的标记。
//!
//! 握手与流转（两侧对称，消息都是同一条 cut text 消息的不同正文）：
//!
//! ```text
//! 客户端 ── SetEncodings（含 ENCODING_EXTENDED_CLIPBOARD）──▶ 服务器
//! 服务器 ── Caps（我支持哪些格式/动作、每个格式收多大）─────▶ 客户端
//! 客户端 ── Caps（同上，回敬一份）────────────────────────▶ 服务器
//! 任一侧 ── Notify（我手上有新文本了）────────────────────▶ 对侧
//! 对侧   ── Request（给我）───────────────────────────────▶ 这一侧
//! 这一侧 ── Provide（格式 + zlib 压缩的数据块）─────────────▶ 对侧
//! ```
//!
//! 只有**对端先递过 Caps** 之后才能用扩展：没实现这个扩展的服务器对上面那条
//! 消息一无所知，收到一个最高位置位的长度只会当成天文数字的 Latin-1 正文。
//! 什么时候发 Caps、什么时候退回经典路径是调用方的策略；这里只负责字节。
//!
//! 读进来的东西一律当作不可信：正文超过上限就流式丢掉（**绝不**让一条剪贴板
//! 消息把会话带下水），解压出来的每一块也各自封顶（防 zip 炸弹）。

use std::io::Read as _;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::limits;
use crate::VncError;

/// 扩展剪贴板的伪编码号，`SetEncodings` 里报出去的值。RealVNC / TigerVNC /
/// noVNC 用的都是这一个（`0xc0a1e5ce`，一个随机挑的魔数，不是负数区间）。
pub const ENCODING_EXTENDED_CLIPBOARD: u32 = 0xc0a1e5ce;

/// 正文里的格式位（flags 低 16 位）。
pub const FORMAT_TEXT: u32 = 1 << 0;
pub const FORMAT_RTF: u32 = 1 << 1;
pub const FORMAT_HTML: u32 = 1 << 2;
pub const FORMAT_DIB: u32 = 1 << 3;
pub const FORMAT_FILES: u32 = 1 << 4;

/// 正文里的动作位（flags 高 8 位）。
pub const ACTION_CAPS: u32 = 1 << 24;
pub const ACTION_REQUEST: u32 = 1 << 25;
pub const ACTION_PEEK: u32 = 1 << 26;
pub const ACTION_NOTIFY: u32 = 1 << 27;
pub const ACTION_PROVIDE: u32 = 1 << 28;

/// 格式位与动作位各自的那一段。
pub const FORMAT_MASK: u32 = 0x0000_ffff;
pub const ACTION_MASK: u32 = 0xff00_0000;

/// 单块数据的字节上限，与经典路径的 `limits::MAX_TEXT` 同一档。
const MAX_CHUNK: usize = limits::MAX_TEXT;

/// 扩展正文的字节上限：小过它的整条读进内存，大过它的一路丢掉，
/// 再大（[`limits::MAX_COMPRESSED`] 往上）就当协议错 —— 那已经是敌意流量，
/// 陪它读完不值得。
const MAX_KEPT: usize = limits::MAX_TEXT;

/// 对端 Caps 里报出来的能力：我支持哪些格式、哪些动作，每个格式愿意收多大。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caps {
    /// 支持的格式，见 [`FORMAT_TEXT`] 这一族。
    pub formats: u32,
    /// 支持的动作，见 [`ACTION_CAPS`] 这一族。Caps 这一条消息自己就靠
    /// `ACTION_CAPS` 位认出来，所以对端报过来的这一份里必然含它，本侧发出去
    /// 时也会自动补上。
    pub actions: u32,
    /// 每个格式愿意接收的字节上限，元素是 (格式位, 上限)。0 表示没声明。
    pub limits: Vec<(u32, u32)>,
}

impl Caps {
    /// 某个格式的接收上限。没报过就是 `None`（当"没声明"用，不当作 0）。
    pub fn limit(&self, format: u32) -> Option<u32> {
        self.limits
            .iter()
            .find(|(known, _)| *known == format)
            .map(|(_, limit)| *limit)
    }
}

/// 一条扩展剪贴板消息的正文。
///
/// 这个类型既当读出来的事件、也当要发出去的命令：`Caps` / `Notify` / `Request` /
/// `Peek` / `Provide` 五个动作两侧通用，方向和上是哪条消息由收发两端各自决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtendedClipboard {
    /// 报出自己的能力（握手的头一条，之后才知道该走哪条路）。
    Caps(Caps),
    /// "把你手上的文本给我"。
    Request { formats: u32 },
    /// "你手上有多大"（只问大小，不要数据）。
    Peek { formats: u32 },
    /// "我手上有新东西了"（随后对侧来 Request）。
    Notify { formats: u32 },
    /// 数据本体：`(格式位, 解压后的字节)`。只有对侧 Request 过、或者对侧刚
    /// Notify 过才该发。
    Provide { items: Vec<(u32, Vec<u8>)> },
    /// 读不懂（动作位不认识、正文截断、解压不出来、超限）：这一条丢掉，
    /// 会话照常。写出去是不可能写这个的，写它只会报错。
    Ignored,
}

/// 读一条扩展剪贴板消息的正文：3 个 padding 与 4 字节长度已经读掉了，
/// `length` 就是那个长度字段（最高位置位才走到这里）。
pub(crate) async fn read_message<S>(
    reader: &mut S,
    length: u32,
) -> Result<ExtendedClipboard, VncError>
where
    S: AsyncRead + Unpin,
{
    // 长度是负数，正文大小取它的绝对值。
    let magnitude = (length as i32).unsigned_abs() as usize;

    if magnitude > MAX_KEPT {
        if magnitude > limits::MAX_COMPRESSED {
            return Err(VncError::General(
                "VNC extended clipboard exceeds size limit".into(),
            ));
        }
        // 太大：读完丢掉，流里的位置对得上，后面的消息才不会错位。
        discard(reader, magnitude).await?;
        tracing::warn!("VNC extended clipboard payload of {magnitude} bytes discarded");
        return Ok(ExtendedClipboard::Ignored);
    }

    let mut body = vec![0; magnitude];
    reader.read_exact(&mut body).await?;
    Ok(parse_body(&body))
}

/// 把 `n` 个字节从流里读掉、不留。读不满就是流断了，照常报错。
async fn discard<S>(reader: &mut S, n: usize) -> Result<(), VncError>
where
    S: AsyncRead + Unpin,
{
    let mut scratch = [0u8; 8192];
    let mut left = n;
    while left > 0 {
        let take = left.min(scratch.len());
        reader.read_exact(&mut scratch[..take]).await?;
        left -= take;
    }
    Ok(())
}

/// 正文的字节 → 消息。`flags` 不够长、动作位不认识、解压不出来，一律
/// [`ExtendedClipboard::Ignored`] —— 这是条不该死人的消息。
fn parse_body(body: &[u8]) -> ExtendedClipboard {
    let Some((flags, rest)) = body.split_first_chunk::<4>() else {
        tracing::warn!("VNC extended clipboard shorter than its flags word");
        return ExtendedClipboard::Ignored;
    };
    let flags = u32::from_be_bytes(*flags);
    let formats = flags & FORMAT_MASK;
    let actions = flags & ACTION_MASK;

    // Caps 那一档的动作位里还捎带着"我支持哪些动作"（对端扫的就是这一段），
    // 所以它不能按等号认，得按位认；其余四条各自只占一个位。
    if actions & ACTION_CAPS != 0 {
        // 每个列出来的格式跟着一个 U32：对侧愿意收多大。
        let mut limits = Vec::new();
        let mut rest = rest;
        for bit in 0..16 {
            let format = 1u32 << bit;
            if formats & format == 0 {
                continue;
            }
            let Some((size, tail)) = rest.split_first_chunk::<4>() else {
                tracing::warn!("VNC extended clipboard caps truncated");
                return ExtendedClipboard::Ignored;
            };
            limits.push((format, u32::from_be_bytes(*size)));
            rest = tail;
        }
        return ExtendedClipboard::Caps(Caps {
            formats,
            actions,
            limits,
        });
    }

    match actions {
        ACTION_REQUEST => ExtendedClipboard::Request { formats },
        ACTION_PEEK => ExtendedClipboard::Peek { formats },
        ACTION_NOTIFY => ExtendedClipboard::Notify { formats },
        ACTION_PROVIDE => parse_provide(formats, rest),
        // 多个动作位叠在一起、或者没见过的动作：读不懂就不要猜。
        _ => {
            tracing::warn!("VNC extended clipboard with unknown action {actions:#x}");
            ExtendedClipboard::Ignored
        }
    }
}

/// Provide 的正文是 zlib 流：按 `formats` 里置位的顺序，每个格式一个 U32 长度
/// 跟在后面的数据块。文本按 UTF-8（末尾可能带一个 NUL）。
fn parse_provide(formats: u32, compressed: &[u8]) -> ExtendedClipboard {
    let mut inflater = flate2::read::ZlibDecoder::new(compressed);
    let mut items = Vec::new();

    for bit in 0..16 {
        let format = 1u32 << bit;
        if formats & format == 0 {
            continue;
        }
        let mut size = [0u8; 4];
        if inflater.read_exact(&mut size).is_err() {
            tracing::warn!("VNC extended clipboard provide truncated");
            return ExtendedClipboard::Ignored;
        }
        let size = u32::from_be_bytes(size) as usize;
        if size > MAX_CHUNK {
            tracing::warn!("VNC extended clipboard chunk of {size} bytes rejected");
            return ExtendedClipboard::Ignored;
        }
        let mut data = vec![0; size];
        if inflater.read_exact(&mut data).is_err() {
            tracing::warn!("VNC extended clipboard provide truncated");
            return ExtendedClipboard::Ignored;
        }
        items.push((format, data));
    }

    if items.is_empty() {
        tracing::warn!("VNC extended clipboard provide with no formats");
        return ExtendedClipboard::Ignored;
    }
    ExtendedClipboard::Provide { items }
}

/// 一条消息 → 线路上的一整条 ClientCutText：类型字节、padding、负数长度、
/// flags，然后是各动作自己的正文。
pub(crate) async fn write_message<S>(
    message: &ExtendedClipboard,
    writer: &mut S,
) -> Result<(), VncError>
where
    S: AsyncWrite + Unpin,
{
    let (flags, body) = body_of(message)?;
    // 4（flags）+ 正文，长度不该顶到 i32 的头上。
    let payload_len = 4usize
        .checked_add(body.len())
        .filter(|len| *len <= i32::MAX as usize)
        .ok_or_else(|| VncError::General("VNC extended clipboard too large".into()))?;

    let mut header = vec![6u8, 0, 0, 0];
    // 长度取负就是扩展标记。
    header.extend_from_slice(&(-(payload_len as i32)).to_be_bytes());
    writer.write_all(&header).await?;
    writer.write_all(&flags.to_be_bytes()).await?;
    writer.write_all(&body).await?;
    Ok(())
}

/// 消息 → `(flags, flags 之后的正文)`。
fn body_of(message: &ExtendedClipboard) -> Result<(u32, Vec<u8>), VncError> {
    match message {
        ExtendedClipboard::Caps(caps) => {
            let mut body = Vec::new();
            for bit in 0..16 {
                let format = 1u32 << bit;
                if caps.formats & format == 0 {
                    continue;
                }
                body.extend_from_slice(&caps.limit(format).unwrap_or(0).to_be_bytes());
            }
            // 动作位这一段既要标出"这是 Caps"，也要把本侧支持的动作一并报出去。
            Ok((
                (ACTION_CAPS | caps.actions) & ACTION_MASK | caps.formats,
                body,
            ))
        }
        ExtendedClipboard::Request { formats } => Ok((ACTION_REQUEST | formats, Vec::new())),
        ExtendedClipboard::Peek { formats } => Ok((ACTION_PEEK | formats, Vec::new())),
        ExtendedClipboard::Notify { formats } => Ok((ACTION_NOTIFY | formats, Vec::new())),
        ExtendedClipboard::Provide { items } => {
            // 数据块按格式位从小到大排 —— 解压那头就是按 flags 的位序读的，
            // 顺序对不上会把两块数据接错。
            let mut items = items.clone();
            items.sort_by_key(|(format, _)| *format);

            let mut formats = 0u32;
            let mut plain = Vec::new();
            for (format, data) in &items {
                if format.count_ones() != 1 {
                    return Err(VncError::General(format!(
                        "VNC clipboard format {format:#x} is not a single bit"
                    )));
                }
                formats |= format;
                plain.extend_from_slice(&(data.len() as u32).to_be_bytes());
                plain.extend_from_slice(data);
            }

            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            std::io::Write::write_all(&mut encoder, &plain)
                .map_err(|error| VncError::General(format!("VNC clipboard zlib: {error}")))?;
            let compressed = encoder
                .finish()
                .map_err(|error| VncError::General(format!("VNC clipboard zlib: {error}")))?;
            Ok((ACTION_PROVIDE | formats, compressed))
        }
        ExtendedClipboard::Ignored => Err(VncError::General(
            "VNC clipboard message cannot be sent".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn round_trip(message: ExtendedClipboard) -> ExtendedClipboard {
        let mut wire = Vec::new();
        write_message(&message, &mut wire).await.unwrap();
        // 类型字节 + padding 先吃掉，长度按负数读出来 —— 和读侧同一条路。
        assert_eq!(wire[0], 6);
        let length = u32::from_be_bytes(wire[4..8].try_into().unwrap());
        assert_eq!(length & 0x8000_0000, 0x8000_0000, "长度该是负的");
        let mut reader = &wire[8..];
        read_message(&mut reader, length).await.unwrap()
    }

    #[tokio::test]
    async fn caps_carry_formats_actions_and_limits() {
        let caps = Caps {
            formats: FORMAT_TEXT | FORMAT_HTML,
            actions: ACTION_CAPS | ACTION_REQUEST | ACTION_NOTIFY | ACTION_PROVIDE,
            limits: vec![(FORMAT_TEXT, 1 << 20), (FORMAT_HTML, 0)],
        };
        let got = round_trip(ExtendedClipboard::Caps(caps.clone())).await;
        assert_eq!(got, ExtendedClipboard::Caps(caps));
    }

    #[tokio::test]
    async fn provide_text_survives_zlib() {
        let text = "你好，剪贴板 café".as_bytes().to_vec();
        let got = round_trip(ExtendedClipboard::Provide {
            items: vec![(FORMAT_TEXT, text.clone())],
        })
        .await;
        match got {
            ExtendedClipboard::Provide { items } => {
                assert_eq!(items, vec![(FORMAT_TEXT, text)]);
            }
            other => panic!("应读回一条 Provide，实际 {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_actions_round_trip() {
        for message in [
            ExtendedClipboard::Notify {
                formats: FORMAT_TEXT,
            },
            ExtendedClipboard::Request {
                formats: FORMAT_TEXT,
            },
            ExtendedClipboard::Peek {
                formats: FORMAT_TEXT,
            },
        ] {
            assert_eq!(round_trip(message.clone()).await, message);
        }
    }

    #[tokio::test]
    async fn malformed_payloads_are_ignored_not_fatal() {
        // 只有 flags、没有动作位；动作位不认识；provide 不是 zlib。
        let no_action: Vec<u8> = vec![0, 0, 1, 0];
        let unknown_action: Vec<u8> = (1u32 << 31).to_be_bytes().to_vec();
        let not_zlib: Vec<u8> = {
            let mut body = (ACTION_PROVIDE | FORMAT_TEXT).to_be_bytes().to_vec();
            body.extend_from_slice(b"not zlib at all");
            body
        };
        for body in [no_action, unknown_action, not_zlib] {
            let length = (body.len() as i32).wrapping_neg() as u32;
            let mut reader = &body[..];
            assert_eq!(
                read_message(&mut reader, length).await.unwrap(),
                ExtendedClipboard::Ignored,
            );
        }
    }

    #[tokio::test]
    async fn an_oversized_payload_is_drained_so_the_stream_stays_aligned() {
        let huge = MAX_KEPT + 16;
        let mut stream = Vec::new();
        stream.extend((huge as i32).wrapping_neg().to_be_bytes());
        stream.extend(vec![0u8; huge]);
        stream.extend(b"after");

        let length = u32::from_be_bytes(stream[..4].try_into().unwrap());
        let mut reader = &stream[4..];
        assert_eq!(
            read_message(&mut reader, length).await.unwrap(),
            ExtendedClipboard::Ignored,
        );
        assert_eq!(reader, b"after", "超限正文该读干净，后面的字节不能错位");
    }

    #[test]
    fn the_pseudo_encoding_number_matches_the_constant() {
        assert_eq!(
            u32::from(crate::VncEncoding::ExtendedClipboardPseudo),
            ENCODING_EXTENDED_CLIPBOARD,
        );
    }
}
