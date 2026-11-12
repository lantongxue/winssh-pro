//! macOS VideoToolbox H.264 硬件解码器：实现 `ironrdp-egfx` 的 [`H264Decoder`]，
//! 给 GFX 管线的 AVC420/AVC444 提供 YUV420 平面（AVC444 的双流拼合与
//! 色彩转换由 egfx 自己做，这里只出平面）。
//!
//! 会话是懒建的：码流里第一次凑齐 SPS/PPS 才建 `VTDecompressionSession`，
//! 参数集变了就重建。解码走同步路径（`decode_frame` 不带异步旗标，回调在
//! 返回前完成），与 egfx 的同步 trait 契约一致；硬解不可用时 VideoToolbox
//! 自己落回软解，再不行返回 Err，由能力协商降档，不断会话。
//!
//! 输出固定要 NV12 全范围（`kCVPixelFormatType_420YpCbCr8BiPlanarFullRange`），
//! 对齐 MS-RDPEGFX AVC 流的 BT.709 全范围口径；拷平面时把交错的 UV 拆成
//! egfx 要的 U、V 两个独立平面。

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use ironrdp_egfx::decode::{DecodedFrame, DecoderError, DecoderResult, H264Decoder, Yuv420Frame};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained};
use objc2_core_media::{
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMSampleTimingInfo, CMTime,
    CMVideoFormatDescription, CMVideoFormatDescriptionCreateFromH264ParameterSets,
    kCMBlockBufferAssureMemoryNowFlag, kCMTimeInvalid,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
    CVPixelBufferGetHeight, CVPixelBufferGetPlaneCount, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
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

        // 硬解开关（不可用时 VideoToolbox 自动落软解）+ NV12 全范围输出。
        let spec = CFDictionary::from_slices(
            &[unsafe { kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder }],
            &[CFBoolean::new(true)],
        );
        let pixel_format =
            CFNumber::new_i32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange as i32);
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

    /// 把解码出的 NV12 拷成 egfx 要的 Y/U/V 平面。
    fn copy_planes(buffer: &CVPixelBuffer) -> DecoderResult<Yuv420Frame> {
        // SAFETY: 只读锁；所有平面访问都在锁内，返回前解锁。
        let status =
            unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        if status != 0 {
            return Err(DecoderError::msg(format!(
                "CVPixelBufferLockBaseAddress failed: {status}"
            )));
        }
        let result = unsafe { Self::copy_planes_locked(buffer) };
        // SAFETY: 与上面的只读锁配对。
        unsafe { CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        result
    }

    /// # Safety
    /// 调用方已持有 `buffer` 的只读锁。
    unsafe fn copy_planes_locked(buffer: &CVPixelBuffer) -> DecoderResult<Yuv420Frame> {
        // SAFETY: 锁内读取像素缓冲的几何信息。
        let (width, height, planes) = (
            CVPixelBufferGetWidth(buffer),
            CVPixelBufferGetHeight(buffer),
            CVPixelBufferGetPlaneCount(buffer),
        );
        if planes != 2 {
            return Err(DecoderError::msg("expected two-planar NV12 output"));
        }
        if width == 0 || height == 0 || width > usize::from(u16::MAX) || height > usize::from(u16::MAX)
        {
            return Err(DecoderError::msg("decoded picture size out of range"));
        }
        // SAFETY: 锁内取平面基址与行距。
        let (y_ptr, y_stride, uv_ptr, uv_stride) = (
            CVPixelBufferGetBaseAddressOfPlane(buffer, 0).cast::<u8>(),
            CVPixelBufferGetBytesPerRowOfPlane(buffer, 0),
            CVPixelBufferGetBaseAddressOfPlane(buffer, 1).cast::<u8>(),
            CVPixelBufferGetBytesPerRowOfPlane(buffer, 1),
        );
        if y_ptr.is_null() || uv_ptr.is_null() {
            return Err(DecoderError::msg("null pixel buffer plane"));
        }
        let mut y = Vec::with_capacity(width * height);
        for row in 0..height {
            // SAFETY: 锁内、行在画面内，行距不小于宽度。
            y.extend_from_slice(unsafe { std::slice::from_raw_parts(y_ptr.add(row * y_stride), width) });
        }
        let chroma_width = width.div_ceil(2);
        let chroma_height = height.div_ceil(2);
        let mut u = Vec::with_capacity(chroma_width * chroma_height);
        let mut v = Vec::with_capacity(chroma_width * chroma_height);
        for row in 0..chroma_height {
            // SAFETY: 锁内、行在色度平面内；NV12 的 UV 交错，每行 2×色度宽。
            let uv_row = unsafe {
                std::slice::from_raw_parts(uv_ptr.add(row * uv_stride), chroma_width * 2)
            };
            for pair in uv_row.as_chunks::<2>().0 {
                u.push(pair[0]);
                v.push(pair[1]);
            }
        }
        Yuv420Frame::new(
            width as u16,
            height as u16,
            y,
            width,
            u,
            v,
            chroma_width,
        )
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
        // RGBA 兜底：egfx 在 supports_yuv420 时走 decode_yuv420 自己做色彩转换。
        Ok(self.decode_yuv420(data)?.to_rgba())
    }

    fn decode_yuv420(&mut self, data: &[u8]) -> DecoderResult<Yuv420Frame> {
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
        Self::copy_planes(&buffer)
    }

    fn supports_yuv420(&self) -> bool {
        true
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