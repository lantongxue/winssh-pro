//! RDP 图形更新适配：包络、MPPC、分片与位图源裁剪，像素仍由 IronRDP 解码。
//!
//! ActiveStage 的 GraphicsUpdate 是损伤矩形，不是桌面帧；它的公开输出没有
//! BeginFrame。这里用公开 PDU API 在送入 ActiveStage 前保留 SurfaceCommands
//! 的 Begin/End，并把每个完成的更新交给调用方。MPPC 只解一次，跨重新激活
//! 保留历史。SlowPath 也使用同一份解压历史与位图裁剪。

use super::bitmap::BitmapNormalizer;
use anyhow::{Result, anyhow, ensure};
use ironrdp::core::{ReadCursor, decode_cursor, encode_vec};
use ironrdp::pdu::fast_path::{
    FastPathHeader, FastPathUpdate, FastPathUpdatePdu, Fragmentation, UpdateCode,
};
use ironrdp::pdu::rdp::client_info::CompressionType;
use ironrdp::pdu::rdp::headers::{ShareControlPdu, ShareDataPdu, decode_share_control};
use ironrdp::pdu::surface_commands::{FrameAction, SurfaceCommand};
use ironrdp::pdu::{mcs, slow_path};
use ironrdp_bulk::{BulkCompressor, CompressionType as BulkType};

// 大于 8K BGRA 整屏，限制损坏的分片序列无限积累。
const MAX_UPDATE: usize = 256 * 1024 * 1024;

pub(super) struct FastPathFrames {
    bulk: Option<BulkCompressor>,
    fragments: Vec<u8>,
    fragment_code: Option<UpdateCode>,
    bitmaps: BitmapNormalizer,
    reported_paths: u16,
}

impl FastPathFrames {
    pub fn has_pending_update(&self) -> bool {
        self.fragment_code.is_some()
    }
    pub fn new(compression: Option<CompressionType>) -> Result<Self> {
        let bulk = compression
            .map(|kind| {
                BulkCompressor::new(match kind {
                    CompressionType::K8 => BulkType::Rdp4,
                    CompressionType::K64 => BulkType::Rdp5,
                    CompressionType::Rdp6 => BulkType::Rdp6,
                    CompressionType::Rdp61 => BulkType::Rdp61,
                })
            })
            .transpose()?;
        Ok(Self {
            bulk,
            fragments: Vec::new(),
            fragment_code: None,
            bitmaps: BitmapNormalizer::default(),
            reported_paths: 0,
        })
    }

    pub fn process(
        &mut self,
        packet: &[u8],
        mut emit: impl FnMut(UpdateCode, &[u8], Option<FrameAction>) -> Result<()>,
    ) -> Result<()> {
        let mut input = ReadCursor::new(packet);
        let header = decode_cursor::<FastPathHeader>(&mut input)?;
        ensure!(
            header.flags.is_empty(),
            "unexpected encrypted FastPath inside TLS"
        );
        while !input.is_empty() {
            let update = decode_cursor::<FastPathUpdatePdu<'_>>(&mut input)?;
            let flag = 1 << update.update_code.as_u8();
            if self.reported_paths & flag == 0 {
                log::debug!(
                    "RDP: FastPath {:?}, compressed={}, fragmented={:?}",
                    update.update_code,
                    update.compression_flags.is_some_and(|f| !f.is_empty()),
                    update.fragmentation
                );
                self.reported_paths |= flag;
            }
            let data =
                if let Some(flags) = update.compression_flags.filter(|flags| !flags.is_empty()) {
                    let bulk = self
                        .bulk
                        .as_mut()
                        .ok_or_else(|| anyhow!("unnegotiated FastPath compression"))?;
                    // FLUSHED / AT_FRONT 同样要进入解压器，即使该段正文未压缩。
                    let flags = u32::from(flags.bits())
                        | u32::from(update.compression_type.map_or(0, |kind| kind.as_u8()));
                    bulk.decompress(update.data, flags)?
                } else {
                    update.data
                };
            match update.fragmentation {
                Fragmentation::Single => {
                    ensure!(
                        self.fragment_code.is_none(),
                        "interrupted FastPath fragments"
                    );
                    emit_update(update.update_code, data, &mut self.bitmaps, &mut emit)?;
                }
                Fragmentation::First => {
                    ensure!(self.fragment_code.is_none(), "nested FastPath fragments");
                    ensure!(data.len() <= MAX_UPDATE, "FastPath update too large");
                    self.fragments.clear();
                    self.fragments.extend_from_slice(data);
                    self.fragment_code = Some(update.update_code);
                }
                Fragmentation::Next | Fragmentation::Last => {
                    ensure!(
                        self.fragment_code == Some(update.update_code),
                        "unmatched FastPath fragment"
                    );
                    ensure!(
                        data.len() <= MAX_UPDATE.saturating_sub(self.fragments.len()),
                        "FastPath update too large"
                    );
                    self.fragments.extend_from_slice(data);
                    if update.fragmentation == Fragmentation::Last {
                        self.fragment_code = None;
                        emit_update(
                            update.update_code,
                            &self.fragments,
                            &mut self.bitmaps,
                            &mut emit,
                        )?;
                        self.fragments.clear();
                    }
                }
            }
        }
        Ok(())
    }

    /// SlowPath 的 Bitmap 正文与 FastPath 相同，也要使用编码宽度裁剪。
    /// 它和 FastPath 共用 MPPC 历史；上游 X224 路径目前未连接这份解压器。
    /// 返回 false 时由原来的 ActiveStage 处理握手、静态通道等消息。
    pub fn process_slow_path(
        &mut self,
        packet: &[u8],
        io_channel: u16,
        mut emit: impl FnMut(UpdateCode, &[u8], Option<FrameAction>) -> Result<()>,
    ) -> Result<bool> {
        let Ok(context) = mcs::decode_send_data_indication(packet) else {
            return Ok(false);
        };
        if context.channel_id != io_channel {
            return Ok(false);
        }
        let context = decode_share_control(context)?;
        let ShareControlPdu::Data(header) = context.pdu else {
            return Ok(false);
        };
        let (data, pointer) = match header.share_data_pdu {
            ShareDataPdu::Update(data) => (data, false),
            ShareDataPdu::Pointer(data) => (data, true),
            _ => return Ok(false),
        };
        let plain = if !header.compression_flags.is_empty() {
            let bulk = self
                .bulk
                .as_mut()
                .ok_or_else(|| anyhow!("unnegotiated SlowPath compression"))?;
            bulk.decompress(
                &data,
                u32::from(header.compression_flags.bits())
                    | u32::from(header.compression_type.as_u8()),
            )?
        } else {
            &data
        };
        let mut input = ReadCursor::new(plain);
        if pointer {
            let update = FastPathUpdate::Pointer(slow_path::decode_slow_path_pointer(&mut input)?);
            emit(UpdateCode::from(&update), &encode_vec(&update)?, None)?;
        } else {
            match slow_path::read_graphics_update_type(&mut input)? {
                slow_path::GraphicsUpdateType::Bitmap => {
                    let normalized = self.bitmaps.normalize(plain)?;
                    emit(
                        UpdateCode::Bitmap,
                        normalized.as_deref().unwrap_or(plain),
                        None,
                    )?;
                }
                slow_path::GraphicsUpdateType::Palette => {
                    emit(UpdateCode::Palette, input.remaining(), None)?;
                }
                slow_path::GraphicsUpdateType::Synchronize => {}
                slow_path::GraphicsUpdateType::Orders => {
                    // 没有协商 Drawing Orders。不能把已经解压的流再走一次旧路径。
                    log::warn!("RDP: received unnegotiated SlowPath drawing orders");
                }
            }
        }
        if self.reported_paths & (1 << 15) == 0 {
            log::debug!(
                "RDP: SlowPath updates, compressed={}",
                !header.compression_flags.is_empty()
            );
            self.reported_paths |= 1 << 15;
        }
        Ok(true)
    }
}

fn emit_update(
    code: UpdateCode,
    data: &[u8],
    bitmaps: &mut BitmapNormalizer,
    emit: &mut impl FnMut(UpdateCode, &[u8], Option<FrameAction>) -> Result<()>,
) -> Result<()> {
    if code == UpdateCode::Bitmap {
        let normalized = bitmaps.normalize(data)?;
        return emit(code, normalized.as_deref().unwrap_or(data), None);
    }
    if code != UpdateCode::SurfaceCommands {
        return emit(code, data, None);
    }
    let mut commands = ReadCursor::new(data);
    // 按命令切开，保证同一 PDU 的 End(A), Begin(B), bits(B) 不会让 B 的
    // 半帧混入 A 的快照。这里只借用原字节，不复制/重编码位图正文。
    while !commands.is_empty() {
        let start = commands.pos();
        let command = decode_cursor::<SurfaceCommand<'_>>(&mut commands)?;
        let boundary = match command {
            SurfaceCommand::FrameMarker(marker) => Some(marker.frame_action),
            _ => None,
        };
        emit(code, &data[start..commands.pos()], boundary)?;
    }
    Ok(())
}
