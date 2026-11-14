//! macOS VideoToolbox H.264 硬件解码器：实现 `ironrdp-egfx` 的 [`H264Decoder`]。
//!
//! 输出 **BGRA32**（`kCVPixelFormatType_32BGRA`），直接交给 egfx 的
//! `decode()` → `DecodedFrame`，不走 YUV 中转：
//!
//! 旧路径（NV12 输出）：VideoToolbox 硬解 → `copy_planes` 拆 Y/U/V（CPU 拷贝）
//! → egfx `to_rgba()` 逐像素 YUV→RGBA（CPU）→ `apply_bitmap` 写画布。
//! 硬解的优势被两步 CPU 转换完全抵消。
//!
//! 新路径（BGRA 输出）：VideoToolbox 硬解 → BGRA `CVPixelBuffer` → 一次
//! `copy_from_slice` → `DecodedFrame` → `apply_bitmap`。
//!
//! 代价：`supports_yuv420()` 返回 `false`，egfx 不 advertise AVC444，
//! 只走 AVC420。AVC444 是否生效取决于服务器策略，不生效时本来就会落回
//! AVC420；放弃 AVC444 换 AVC420 直接出 BGRA，实测收益远大于理论损失。
//!
//! 会话是懒建的：码流里第一次凑齐 SPS/PPS 才建 `VTDecompressionSession`，
//! 参数集变了就重建。解码走同步路径（`decode_frame` 不带异步旗标，回调在
//! 返回前完成），与 egfx 的同步 trait 契约一致；硬解不可用时 VideoToolbox
//! 自己落回软解，再不行返回 Err，由能力协商降档，不断会话。

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use ironrdp_egfx::decode::{DecodedFrame, DecoderError, DecoderResult, H264Decoder};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained};
use objc2_core_media::{
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMSampleTimingInfo, CMTime,
    CMVideoFormatDescription, CMVideoFormatDescriptionCreateFromH264ParameterSets,
    kCMBlockBufferAssureMemoryNowFlag, kCMTimeInvalid,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelFormatType_32BGRA,
};
use objc2_video_toolbox::{
    VTDecompressionOutputCallbackRecord, VTDecompressionSession, VTDecodeFrameFlags,
    VTDecodeInfoFlags, VTSessionSetProperty, kVTDecompressionPropertyKey_RealTime,
    kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder,
};

/// 解码会话与格式描述。CoreFoundation 对象在绑定里被保守地标成 `!Send`；
/// VideoToolbox 的解码会话本身是线程安全的，且本解码器只在 RDP 会话线程
/// 上创建与调用（egfx 同步驱动），随解码器跨线程移动是安全的。
struct SessionState {
    session: CFRetained<VTDecompressionSession>,
    format: CFRetained<CMVideoFormatDescription>,
}

// SAFETY: 见 SessionState 的说明：会话线程安全，且实际只在一条线程上用。
unsafe impl Send for SessionState {}

pub struct VideoToolboxDecoder {
    state: Option<SessionState>,
    /// 最近的参数集（SPS/PPS，去掉起始码）。变化时重建会话。
    parameter_sets: Vec<Vec<u8>>,
    /// AVCC 草稿：4 字节大端长度 + NAL 正文，喂给解码器前拼好。
    avcc: Vec<u8>,
}

/// 同步解码的落点：回调在 `decode_frame` 返回前写它，栈上够用。
struct DecompressionOutput {
    status: i32,
    buffer: Option<CFRetained<CVPixelBuffer>>,
}

/// VideoToolbox 的输出回调。同步解码保证它在 `decode_frame` 返回前完成。
///
/// # Safety
/// `refcon` 必须是调用方栈上的 [`DecompressionOutput`]；`image_buffer` 在回调
/// 期间有效，retain 之后交给调用方释放。
unsafe extern "C-unwind" fn decompression_callback(
    refcon: *mut c_void,
    _source_frame_refcon: *mut c_void,
    status: i32,
    _info_flags: VTDecodeInfoFlags,
    image_buffer: *mut CVPixelBuffer,
    _presentation_time_stamp: CMTime,
    _presentation_duration: CMTime,
) {
    // SAFETY: refcon 由 decode_frame 的调用方传入，指向其栈上的输出结构。
    let output = unsafe { &mut *(refcon as *mut DecompressionOutput) };
    output.status = status;
    if status == 0 && !image_buffer.is_null() {
        // SAFETY: 回调期间 image_buffer 有效；retain 一份带出去，
        // 调用方用完后随 CFRetained 释放。
        output.buffer = Some(unsafe { CFRetained::retain(NonNull::new_unchecked(image_buffer)) });
    }
}

impl VideoToolboxDecoder {
    pub fn new() -> DecoderResult<Self> {
        Ok(Self {
            state: None,
            parameter_sets: Vec::new(),
            avcc: Vec::new(),
        })
    }

    /// 用当前参数集重建格式描述与解码会话。
    fn rebuild_session(&mut self) -> DecoderResult<()> {
        let has_sps = self
            .parameter_sets
            .iter()
            .any(|nal| !nal.is_empty() && nal[0] & 0x1f == 7);
        let has_pps = self
            .parameter_sets
            .iter()
            .any(|nal| !nal.is_empty() && nal[0] & 0x1f == 8);
        if !has_sps || !has_pps {
            return Err(DecoderError::msg("H.264 stream missing SPS or PPS"));
        }
        let mut pointers: Vec<NonNull<u8>> = self
            .parameter_sets
            .iter()
            .map(|nal| NonNull::new(nal.as_ptr().cast_mut()).expect("non-empty NAL"))
            .collect();
        let mut sizes: Vec<usize> = self.parameter_sets.iter().map(Vec::len).collect();
        let mut raw_format: *const CMFormatDescription = ptr::null();
        // SAFETY: 指针与长度表一一对应且在本调用期间有效；输出指针可写。
        let status = unsafe {
            CMVideoFormatDescriptionCreateFromH264ParameterSets(
                None,
                pointers.len(),
                NonNull::new(pointers.as_mut_ptr()).expect("non-empty parameter sets"),
                NonNull::new(sizes.as_mut_ptr()).expect("non-empty parameter sets"),
                4,
                NonNull::from(&mut raw_format),
            )
        };
        if status != 0 || raw_format.is_null() {
            return Err(DecoderError::msg(format!(
                "CMVideoFormatDescriptionCreateFromH264ParameterSets failed: {status}"
            )));
        }
        // SAFETY: 创建成功转移所有权；类型是 H.264 的 CMVideoFormatDescription。
        let format = unsafe {
            CFRetained::from_raw(
                NonNull::new(raw_format.cast_mut().cast::<CMVideoFormatDescription>())
                    .expect("checked non-null"),
            )
        };

        // 硬解开关（不可用时 VideoToolbox 自动落软解）+ BGRA32 输出。
        let spec = CFDictionary::from_slices(
            &[unsafe { kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder }],
            &[CFBoolean::new(true)],
        );
        let pixel_format = CFNumber::new_i32(kCVPixelFormatType_32BGRA as i32);
        let attribute_values: [&CFNumber; 1] = [&pixel_format];
        let attributes = CFDictionary::from_slices(
            &[unsafe { kCVPixelBufferPixelFormatTypeKey }],
            &attribute_values,
        );
        let mut output = DecompressionOutput {
            status: 0,
            buffer: None,
        };
        let record = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(decompression_callback),
            decompressionOutputRefCon: (&mut output as *mut DecompressionOutput).cast::<c_void>(),
        };
        let mut raw_session: *mut VTDecompressionSession = ptr::null_mut();
        // SAFETY: 格式描述有效；两张字典的键值类型正确；回调记录指向栈上输出；
        // 输出指针可写。
        let status = unsafe {
            VTDecompressionSession::create(
                None,
                &format,
                Some(spec.as_opaque()),
                Some(attributes.as_opaque()),
                &record,
                NonNull::from(&mut raw_session),
            )
        };
        if status != 0 || raw_session.is_null() {
            return Err(DecoderError::msg(format!(
                "VTDecompressionSessionCreate failed: {status}"
            )));
        }
        // SAFETY: 创建成功转移所有权。
        let session = unsafe {
            CFRetained::from_raw(NonNull::new(raw_session).expect("checked non-null"))
        };
        // 实时解码：不缓冲、不按显示顺序重排，桌面要的是低延迟。
        // SAFETY: 会话有效，键值类型正确（CFBoolean）。
        unsafe {
            VTSessionSetProperty(
                &session,
                kVTDecompressionPropertyKey_RealTime,
                Some(CFBoolean::new(true)),
            )
        };
        if let Some(old) = self.state.replace(SessionState { session, format }) {
            // SAFETY: 旧会话不再使用，显式拆除。
            unsafe { old.session.invalidate() };
        }
        Ok(())
    }

    /// 把解码出的 BGRA32 拷成 egfx 要的 `DecodedFrame`（RGBA）。
    /// BGRA → RGBA 只需交换 R/B 通道，逐 4 字节组就地交换。
    fn copy_bgra(buffer: &CVPixelBuffer) -> DecoderResult<DecodedFrame> {
        // SAFETY: 只读锁；所有访问都在锁内，返回前解锁。
        let status =
            unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        if status != 0 {
            return Err(DecoderError::msg(format!(
                "CVPixelBufferLockBaseAddress failed: {status}"
            )));
        }
        let result = unsafe { Self::copy_bgra_locked(buffer) };
        // SAFETY: 与上面的只读锁配对。
        unsafe { CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        result
    }

    /// # Safety
    /// 调用方已持有 `buffer` 的只读锁。
    unsafe fn copy_bgra_locked(buffer: &CVPixelBuffer) -> DecoderResult<DecodedFrame> {
        // SAFETY: 锁内读取像素缓冲的几何信息。
        let (width, height) = (
            CVPixelBufferGetWidth(buffer),
            CVPixelBufferGetHeight(buffer),
        );
        if width == 0 || height == 0 || width > usize::from(u16::MAX) || height > usize::from(u16::MAX)
        {
            return Err(DecoderError::msg("decoded picture size out of range"));
        }
        // SAFETY: 锁内取基址与行距。
        let (ptr, stride) = (
            CVPixelBufferGetBaseAddress(buffer).cast::<u8>(),
            CVPixelBufferGetBytesPerRow(buffer),
        );
        if ptr.is_null() {
            return Err(DecoderError::msg("null pixel buffer"));
        }

        // BGRA32 → RGBA：逐行拷出，交换 R/B 通道。
        let rgba_len = width * height * 4;
        let mut rgba = vec![0u8; rgba_len];
        if stride == width * 4 {
            // SAFETY: 锁内、连续 width*height*4 字节可读。
            rgba.copy_from_slice(unsafe { std::slice::from_raw_parts(ptr, rgba_len) });
        } else {
            for row in 0..height {
                let dst = &mut rgba[row * width * 4..(row + 1) * width * 4];
                // SAFETY: 锁内、行在画面内，行距不小于 width*4。
                dst.copy_from_slice(unsafe {
                    std::slice::from_raw_parts(ptr.add(row * stride), width * 4)
                });
            }
        }
        // BGRA → RGBA：交换每 4 字节组的 [0] 和 [2]。
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        Ok(DecodedFrame::new(rgba, width as u32, height as u32))
    }

    /// 解码一帧 H.264 码流，VideoToolbox 输出 BGRA32，拷成 `DecodedFrame`（RGBA）。
    fn decode_bgra(&mut self, data: &[u8]) -> DecoderResult<DecodedFrame> {
        let mut sets_changed = false;
        let mut has_picture = false;
        self.avcc.clear();
        for_each_nal(data, |nal| {
            if nal.is_empty() {
                return;
            }
            match nal[0] & 0x1f {
                kind @ (7 | 8) => {
                    // 同类参数集替换，新的追加；SPS/PPS 进格式描述，不进码流。
                    match self
                        .parameter_sets
                        .iter()
                        .position(|old| !old.is_empty() && old[0] & 0x1f == kind)
                    {
                        Some(index) => {
                            if self.parameter_sets[index] != nal {
                                self.parameter_sets[index] = nal.to_vec();
                                sets_changed = true;
                            }
                        }
                        None => {
                            self.parameter_sets.push(nal.to_vec());
                            sets_changed = true;
                        }
                    }
                }
                9 => {} // AUD：分隔符，不喂解码器。
                _ => {
                    has_picture = true;
                    self.avcc
                        .extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    self.avcc.extend_from_slice(nal);
                }
            }
        });
        if sets_changed || self.state.is_none() {
            self.rebuild_session()?;
        }
        if !has_picture {
            return Err(DecoderError::msg("H.264 payload carries no picture"));
        }
        let state = self
            .state
            .as_ref()
            .ok_or_else(|| DecoderError::msg("no decompression session"))?;

        // AVCC 码流装进 CMBlockBuffer；AssureMemoryNow 让它拷走数据，
        // 草稿缓冲下一帧接着用。
        let mut raw_block: *mut CMBlockBuffer = ptr::null_mut();
        // SAFETY: 草稿缓冲在本调用期间有效；AssureMemoryNow 立即拷贝，
        // 输出指针可写。
        let status = unsafe {
            CMBlockBuffer::create_with_memory_block(
                None,
                self.avcc.as_mut_ptr().cast::<c_void>(),
                self.avcc.len(),
                None,
                ptr::null(),
                0,
                self.avcc.len(),
                kCMBlockBufferAssureMemoryNowFlag,
                NonNull::from(&mut raw_block),
            )
        };
        if status != 0 || raw_block.is_null() {
            return Err(DecoderError::msg(format!(
                "CMBlockBufferCreateWithMemoryBlock failed: {status}"
            )));
        }
        // SAFETY: 创建成功转移所有权。
        let block = unsafe {
            CFRetained::from_raw(NonNull::new(raw_block).expect("checked non-null"))
        };
        let timing = CMSampleTimingInfo {
            duration: unsafe { kCMTimeInvalid },
            presentationTimeStamp: unsafe { kCMTimeInvalid },
            decodeTimeStamp: unsafe { kCMTimeInvalid },
        };
        let sample_size = self.avcc.len();
        let mut raw_sample: *mut CMSampleBuffer = ptr::null_mut();
        let format_ref: &CMFormatDescription = &state.format;
        // SAFETY: 块缓冲与格式描述有效；timing/size 各一条且在本调用期间有效；
        // 输出指针可写。
        let status = unsafe {
            CMSampleBuffer::create_ready(
                None,
                Some(&block),
                Some(format_ref),
                1,
                1,
                &timing,
                1,
                &sample_size,
                NonNull::from(&mut raw_sample),
            )
        };
        if status != 0 || raw_sample.is_null() {
            return Err(DecoderError::msg(format!(
                "CMSampleBufferCreateReady failed: {status}"
            )));
        }
        // SAFETY: 创建成功转移所有权。
        let sample = unsafe {
            CFRetained::from_raw(NonNull::new(raw_sample).expect("checked non-null"))
        };

        let mut output = DecompressionOutput {
            status: 0,
            buffer: None,
        };
        // 不带异步旗标：解码与回调都在本调用内完成，output 栈上安全。
        // SAFETY: 会话与样本缓冲有效；refcon 指向栈上输出；info_flags 传空。
        let status = unsafe {
            state.session.decode_frame(
                &sample,
                VTDecodeFrameFlags::empty(),
                (&mut output as *mut DecompressionOutput).cast::<c_void>(),
                ptr::null_mut(),
            )
        };
        if status != 0 {
            return Err(DecoderError::msg(format!(
                "VTDecompressionSessionDecodeFrame failed: {status}"
            )));
        }
        if output.status != 0 || output.buffer.is_none() {
            return Err(DecoderError::msg(format!(
                "VideoToolbox decode callback failed: {}",
                output.status
            )));
        }
        // 回调已 retain，这里直接接管。
        let buffer = output.buffer.expect("checked above");
        Self::copy_bgra(&buffer)
    }
}

impl Drop for VideoToolboxDecoder {
    fn drop(&mut self) {
        if let Some(state) = &self.state {
            // SAFETY: 会话不再使用，显式拆除。
            unsafe { state.session.invalidate() };
        }
    }
}

impl H264Decoder for VideoToolboxDecoder {
    fn decode(&mut self, data: &[u8]) -> DecoderResult<DecodedFrame> {
        // 直接走 BGRA 路径：VideoToolbox 输出 BGRA32，一次拷贝 + R/B 交换，
        // 不经 YUV 中转。
        Ok(self.decode_bgra(data)?)
    }

    fn supports_yuv420(&self) -> bool {
        // 返回 false：让 egfx 走 decode() 而非 decode_yuv420().to_rgba()，
        // 避免 NV12 → Y/U/V 拆平面 → 逐像素 YUV→RGBA 的两步 CPU 转换。
        // 代价：不 advertise AVC444，只走 AVC420。
        false
    }

    fn wait_until_ready(&mut self) -> bool {
        // 会话懒建且同步，构造即可用；真正的失败在解码时报，由 egfx 降档。
        true
    }

    fn reset(&mut self) {
        if let Some(state) = self.state.take() {
            // SAFETY: 会话不再使用，显式拆除。
            unsafe { state.session.invalidate() };
        }
        self.parameter_sets.clear();
        self.avcc.clear();
    }
}

/// 把码流拆成 NAL 单元（去掉起始码/长度前缀），Annex B 与 4 字节大端
/// 长度前缀（AVCC）两种封装都收 —— 上游文档明确两种都会出现。
fn for_each_nal(data: &[u8], mut visit: impl FnMut(&[u8])) {
    if data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1]) {
        let mut start = None;
        let mut i = 0;
        while i + 3 <= data.len() {
            let start_code = data[i] == 0
                && data[i + 1] == 0
                && (data[i + 2] == 1
                    || (data[i + 2] == 0 && i + 4 <= data.len() && data[i + 3] == 1));
            if start_code {
                let len = if data[i + 2] == 1 { 3 } else { 4 };
                if let Some(s) = start {
                    visit(trim_trailing_zeros(&data[s..i]));
                }
                start = Some(i + len);
                i += len;
            } else {
                i += 1;
            }
        }
        if let Some(s) = start {
            visit(trim_trailing_zeros(&data[s..]));
        }
    } else {
        let mut i = 0;
        while i + 4 <= data.len() {
            let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
            i += 4;
            if len == 0 || i + len > data.len() {
                break;
            }
            visit(&data[i..i + len]);
            i += len;
        }
    }
}

/// Annex B 允许 NAL 后跟 trailing_zero_8bits，不能带进 AVCC 长度里。
fn trim_trailing_zeros(nal: &[u8]) -> &[u8] {
    let mut end = nal.len();
    while end > 0 && nal[end - 1] == 0 {
        end -= 1;
    }
    &nal[..end]
}

#[cfg(test)]
#[path = "tests/videotoolbox.rs"]
mod tests;