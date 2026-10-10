//! 终端流里的 rz/sz 嗅探：认 ZMODEM 握手、剥 `rz\r`、冷却期吞字节。
//!
//! 认的对象是 ZMODEM 的头帧签名：`**\x18B`（hex）、`**\x18A`（bin16）、
//! `**\x18C`（bin32）三种编码，后面跟着的那个帧类型字节才是关键 —— 只有
//! 两个握手帧会开一次新会话：
//!
//! - `ZRQINIT`（0x00，`sz` 发出：远端要发文件，我们是接收方）
//! - `ZSINIT`（0x02，`sz -e` 发出的同一件事，多一段注意串）
//! - `ZRINIT`（0x01，`rz` 发出：远端要收文件，我们是发送方）
//!
//! 其余帧型（`ZACK`、`ZFIN` 这些停在流里的残帧）不当触发器：认到它们说明
//! 上一场会话的尾巴还没淌完，放过去，别让用户收到一个莫名其妙的弹窗。
//!
//! **跨 chunk 的签名要接得上**，所以尾部匹配到一半的候选前缀先扣住不发
//! （最多 6 字节）；扣住的字节在下一段到达时立即结清 —— 接得上就继续等，
//! 接不上就原样吐给屏幕。万一下一段永远不来（对端停在签名中间再没动静），
//! Emulator 侧的定时器会把扣住的字节兜底放出来，见
//! [`crate::terminal::Emulator::zmodem_flush_hold`]。
//!
//! 冷却期（取消/失败之后的一小段）里剩下的字节不能原样进屏幕：对端 `sz`
//! 停手之前已经在路上的数据是二进制垃圾。这里的做法是全吞、只留尾巴，
//! 到期看尾巴"像不像终端文本"再决定放不放 —— 提示符来得早（还在冷却里）
//! 也不会丢。

use std::borrow::Cow;

/// ZMODEM 的转义字节（ZDLE）。签名里必须有一个，也是把普通输出和协议帧
/// 区分开的那一枚控制字节：终端输出里出现裸 `\x18` 大多就是 Ctrl+X。
pub const ZDLE: u8 = 0x18;

/// 冷却期尾巴最多留多少字节。够兜住一个提示符加几行报错，多了没有意义。
const COOLDOWN_TAIL_CAP: usize = 4096;

/// 握手的类别：对端要发（`sz`，我们收）还是对端要收（`rz`，我们发）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerKind {
    /// `ZRQINIT` / `ZSINIT`：远端开始 `sz`，我们是接收方。
    Receive,
    /// `ZRINIT`：远端开始 `rz`，我们是发送方。
    Send,
}

/// 一段被认出来的握手：**从签名开头起**的原始字节。
///
/// 接收方要把这段原样喂给协议机（`ZRQINIT` 就是它要应答的那一帧）；
/// 发送方不用——我们主动送 `ZRQINIT`，对端 `rz` 会应我们 `ZRINIT`。
#[derive(Debug, PartialEq, Eq)]
pub struct Trigger {
    pub kind: TriggerKind,
    pub bytes: Vec<u8>,
}

/// 一次路由的结果：哪一段该进屏幕、哪一段开了一场会话。
#[derive(Debug)]
pub struct Routed<'a> {
    /// 该进屏幕的那一段（命中时已剥掉紧邻签名的 `rz\r`）。
    pub terminal: Cow<'a, [u8]>,
    /// 从这里开始的原始字节归协议。`None` = 这一段里没有握手。
    pub trigger: Option<Trigger>,
}

/// 签名扫描器。只持有一份"候选前缀"的暂存（≤6 字节），其余零拷贝。
#[derive(Debug, Default)]
pub struct Scanner {
    /// 上一段结尾处匹配到一半的候选前缀（`*`、`**`、`**\x18`……）。
    hold: Vec<u8>,
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// 现在扣着候选前缀没有。
    pub fn has_hold(&self) -> bool {
        !self.hold.is_empty()
    }

    /// 放掉扣住的候选前缀，原样交出来（定时兜底：下一段永远没来）。
    pub fn flush(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.hold)
    }

    /// 路由一段远端字节。
    ///
    /// - 没命中：整段（连同上一轮扣住的）交给屏幕；
    /// - 命中：签名之前的交给屏幕，从签名起的装进 [`Routed::trigger`]；
    /// - 匹配到一半：前缀扣住，之前的交给屏幕，下一段接着来。
    pub fn route<'a>(&mut self, bytes: &'a [u8]) -> Routed<'a> {
        if self.hold.is_empty() {
            match scan(bytes) {
                Scan::Clear => Routed {
                    terminal: Cow::Borrowed(bytes),
                    trigger: None,
                },
                Scan::Partial(at) => {
                    self.hold = bytes[at..].to_vec();
                    Routed {
                        terminal: Cow::Borrowed(&bytes[..at]),
                        trigger: None,
                    }
                }
                Scan::Hit(at, kind) => Routed {
                    terminal: Cow::Borrowed(trim_rz(&bytes[..at])),
                    trigger: Some(Trigger {
                        kind,
                        bytes: bytes[at..].to_vec(),
                    }),
                },
            }
        } else {
            // 接上上一轮扣住的前缀一起看。拷贝一份是值得的：这只发生在
            // 签名被切在 chunk 边界上的时候，一次几个字节。
            let mut joined = std::mem::take(&mut self.hold);
            joined.extend_from_slice(bytes);
            match scan(&joined) {
                Scan::Clear => Routed {
                    terminal: Cow::Owned(joined),
                    trigger: None,
                },
                Scan::Partial(at) => {
                    self.hold = joined[at..].to_vec();
                    Routed {
                        terminal: Cow::Owned(joined[..at].to_vec()),
                        trigger: None,
                    }
                }
                Scan::Hit(at, kind) => Routed {
                    terminal: Cow::Owned(trim_rz(&joined[..at]).to_vec()),
                    trigger: Some(Trigger {
                        kind,
                        bytes: joined[at..].to_vec(),
                    }),
                },
            }
        }
    }
}

/// 扫描的结果。
enum Scan {
    /// 整段里没有候选。
    Clear,
    /// 尾部从 `at` 起是个匹配到一半的候选前缀，扣住。
    Partial(usize),
    /// `at` 起是一枚能开场的握手。
    Hit(usize, TriggerKind),
}

/// 从头到尾找候选签名。最多需要在尾部往回看 6 个字节（`**\x18B` + 两位
/// 十六进制），所以部分匹配只能在尾部发生。
fn scan(bytes: &[u8]) -> Scan {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'*' {
            i += 1;
            continue;
        }
        let s = &bytes[i..];
        // `*` 之后还可能是 `*`、`\x18`、编码字母、帧类型字节、两位十六进制
        // —— 一层层往前看，任何一层不成立就跳过去接着扫。
        if s.len() < 2 {
            return Scan::Partial(i);
        }
        if s[1] != b'*' {
            i += 1;
            continue;
        }
        if s.len() < 3 {
            return Scan::Partial(i);
        }
        if s[2] != ZDLE {
            i += 1;
            continue;
        }
        if s.len() < 4 {
            return Scan::Partial(i);
        }
        let frame_type = match s[3] {
            b'A' | b'C' => {
                // bin16 / bin32：帧类型字节跟在编码字母后面。
                if s.len() < 5 {
                    return Scan::Partial(i);
                }
                s[4]
            }
            b'B' => {
                // hex：帧类型字节是两位十六进制。
                if s.len() < 6 {
                    return Scan::Partial(i);
                }
                let (high, low) = (hex_value(s[4]), hex_value(s[5]));
                match (high, low) {
                    (Some(high), Some(low)) => (high << 4) | low,
                    _ => {
                        i += 1;
                        continue;
                    }
                }
            }
            _ => {
                i += 1;
                continue;
            }
        };
        match frame_type {
            // ZRQINIT / ZSINIT：远端要发（sz）。
            0x00 | 0x02 => return Scan::Hit(i, TriggerKind::Receive),
            // ZRINIT：远端要收（rz）。
            0x01 => return Scan::Hit(i, TriggerKind::Send),
            // 其它帧型：停在流里的上一场会话的尾巴，不是新会话的开场。
            _ => {
                i += 1;
                continue;
            }
        }
    }
    Scan::Clear
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 剥掉签名前面那段 `rz\r`。
///
/// 远端 `sz` 起手就是这五个字节：先打一行 `rz`（老终端的"提醒"惯例，好让
/// 已经进过 ZMODEM 模式的终端先退出来），紧接着才是头帧。只剥紧贴签名的
/// 那一段，之前真有一行 `rz` 输出的话就留着。
fn trim_rz(bytes: &[u8]) -> &[u8] {
    const TAILS: [&[u8]; 4] = [b"rz\r\n", b"rz\r", b"rz\n", b"rz"];
    for tail in TAILS {
        if bytes.ends_with(tail) {
            return &bytes[..bytes.len() - tail.len()];
        }
    }
    bytes
}

/// 冷却期的吞字节器：全吞，只留尾巴。
///
/// 取消/失败之后对端停手前已经在路上的数据不能进屏幕（二进制垃圾），但
/// 紧接着回来的提示符、报错行是这个窗口里最该看见的东西。尾巴到期按
/// [`release`](Self::release) 的判据放行。
#[derive(Debug, Default)]
pub struct CooledTail {
    tail: Vec<u8>,
}

impl CooledTail {
    pub fn new() -> Self {
        Self::default()
    }

    /// 吞掉一段，把尾巴（最近的字节）留住。
    pub fn swallow(&mut self, bytes: &[u8]) {
        self.tail.extend_from_slice(bytes);
        if self.tail.len() > COOLDOWN_TAIL_CAP {
            let drop = self.tail.len() - COOLDOWN_TAIL_CAP;
            self.tail.drain(..drop);
        }
    }

    /// 冷却到期：把尾巴里最后一枚"不可能出现在文本里"的控制字节之后的
    /// 那一段交出来（喂给屏幕），其余丢掉。
    ///
    /// 取消时来的通常是 `[二进制垃圾][报错行][提示符]`：末段正是最该看见的
    /// 东西。判据是"最后一枚非法控制字节"：`\r`、`\n`、`\t`、`\x1b`、
    /// `\x08`、`\x07` 算文本的一部分；协议残渣几乎总会带一枚低控制字节
    /// —— ZDATA 的正文里满是被 ZDLE 转义的表现（裸 `\x18`）和不该出现的
    /// 原始控制码。代价是纯垃圾尾部的可打印尾巴（如果有）会漏出去，一瞬的
    /// 花脸比丢掉提示符轻。
    pub fn release(&mut self) -> Option<Vec<u8>> {
        let tail = std::mem::take(&mut self.tail);
        let start = tail
            .iter()
            .rposition(|&byte| is_protocol_residue(byte))
            .map_or(0, |at| at + 1);
        let suffix = &tail[start..];
        // 接收收场时对端（lrzsz 的 `sz`）会补一个 `OO`（"over and out"）：
        // 它是协议的道别，不是给屏幕看的东西。只认"后面跟着换行或者到
        // 头了"的那种 —— 一条恰好叫 `OOPS ...` 的报错行不能被误伤。
        let suffix = match suffix {
            [b'O', b'O', rest @ ..] if rest.is_empty() || matches!(rest[0], b'\r' | b'\n') => rest,
            _ => suffix,
        };
        match suffix.is_empty() {
            true => None,
            false => Some(suffix.to_vec()),
        }
    }
}

/// 这枚字节能不能出现在终端文本里。制表控制符（回车、换行、制表、Esc、
/// 退格、响铃）算可以；其余低位控制码算协议残渣。
fn is_protocol_residue(byte: u8) -> bool {
    byte < 0x20 && !matches!(byte, b'\r' | b'\n' | b'\t' | 0x1b | 0x08 | 0x07)
}

#[cfg(test)]
#[path = "../tests/zmodem/sniff.rs"]
mod tests;
