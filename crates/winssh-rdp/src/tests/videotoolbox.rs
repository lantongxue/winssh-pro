use super::*;

#[test]
fn annexb_start_codes_split_nals_and_drop_trailing_zeros() {
    // 3 字节与 4 字节起始码混用，末尾带 trailing_zero_8bits。
    let stream = [
        vec![0, 0, 0, 1, 0x67, 0x42, 0x00, 0x0a], // SPS
        vec![0, 0, 1, 0x68, 0xce, 0x3c, 0x80],    // PPS
        vec![0, 0, 1, 0x65, 0x11, 0x22, 0, 0],    // IDR + 两个尾零
    ]
    .concat();
    let mut nals = Vec::new();
    for_each_nal(&stream, |nal| nals.push(nal.to_vec()));
    assert_eq!(
        nals,
        vec![
            vec![0x67, 0x42, 0x00, 0x0a],
            vec![0x68, 0xce, 0x3c, 0x80],
            vec![0x65, 0x11, 0x22],
        ]
    );
}

#[test]
fn avcc_length_prefix_splits_nals() {
    let mut stream = Vec::new();
    for nal in [
        [0x67u8, 0x42].as_slice(),
        [0x68, 0xce].as_slice(),
        [0x65, 0x11, 0x22].as_slice(),
    ] {
        stream.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        stream.extend_from_slice(nal);
    }
    let mut nals = Vec::new();
    for_each_nal(&stream, |nal| nals.push(nal.to_vec()));
    assert_eq!(nals, vec![vec![0x67, 0x42], vec![0x68, 0xce], vec![0x65, 0x11, 0x22]]);
}

#[test]
fn truncated_avcc_length_stops_without_panicking() {
    // 长度字段说有 100 字节，实际只有 2 字节：丢弃残段，不越界。
    let stream = [0, 0, 0, 100, 0x65, 0x11];
    let mut nals = Vec::new();
    for_each_nal(&stream, |nal| nals.push(nal.to_vec()));
    assert!(nals.is_empty());
}

/// 真实码流实解：环境敏感（要 VideoToolbox 与一份 H.264 样本），默认忽略。
/// 样本用环境变量 WINSSH_H264_SAMPLE 指一个 Annex B 裸流文件，例如：
/// ffmpeg -f lavfi -i color=black:size=64x64:duration=0.2 -c:v libx264 -f h264 sample.264
#[test]
#[ignore = "需要 VideoToolbox 与 WINSSH_H264_SAMPLE 样本，按需手动跑"]
fn videotoolbox_decodes_a_real_stream_to_yuv420() {
    let path = std::env::var("WINSSH_H264_SAMPLE").expect("WINSSH_H264_SAMPLE 指向 .264 样本");
    let data = std::fs::read(path).expect("read sample");
    let mut decoder = VideoToolboxDecoder::new().expect("decoder");
    assert!(decoder.wait_until_ready());
    assert!(decoder.supports_yuv420());
    // 整份样本一次喂入：参数集与所有帧都在里面，最后一帧必须解得出来。
    let frame = decoder.decode_yuv420(&data).expect("decode");
    assert_eq!((frame.width(), frame.height()), (64, 64));
    assert!(!frame.luma_row(0).unwrap().is_empty());
    // 复位后再解同一段，验证会话重建。
    decoder.reset();
    let again = decoder.decode_yuv420(&data).expect("decode after reset");
    assert_eq!((again.width(), again.height()), (64, 64));
}