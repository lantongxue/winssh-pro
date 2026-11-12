use super::*;
use ironrdp_dvc::DvcProcessor;
use ironrdp_egfx::client::GraphicsPipelineClient;
use ironrdp_egfx::decode::{DecodedFrame, DecoderResult, H264Decoder};
use ironrdp_egfx::pdu::{CapabilityVersion, GfxPdu};

/// 只声明能力、不出像素的假解码器：协商测试不需要真解。
struct FakeDecoder {
    yuv: bool,
}

impl H264Decoder for FakeDecoder {
    fn decode(&mut self, _data: &[u8]) -> DecoderResult<DecodedFrame> {
        unreachable!("协商测试不解码")
    }

    fn supports_yuv420(&self) -> bool {
        self.yuv
    }
}

/// 通道打开时客户端 advertise 出去的能力版本表。
fn advertised_versions(decoder: Option<Box<dyn H264Decoder>>) -> Vec<u32> {
    let mut client = GraphicsPipelineClient::new(Box::new(GfxHandler), decoder);
    let messages = client.start(1005).expect("start");
    let mut bytes = Vec::new();
    for message in &messages {
        bytes.extend(ironrdp_core::encode_vec(message.as_ref()).expect("encode"));
    }
    let GfxPdu::CapabilitiesAdvertise(advertise) =
        ironrdp_core::decode::<GfxPdu>(&bytes).expect("decode advertise")
    else {
        panic!("expected CapabilitiesAdvertise")
    };
    advertise.0.iter().map(|cap| cap.version.0).collect()
}

#[test]
fn gfx_advertises_avc444_only_with_a_yuv_plane_decoder() {
    // 出 YUV 平面的解码器：AVC444 档（10.x）全报，最高 10.7。
    let versions = advertised_versions(Some(Box::new(FakeDecoder { yuv: true })));
    assert_eq!(versions.first(), Some(&CapabilityVersion::V10_7.0));
    assert!(versions.contains(&CapabilityVersion::V8_1.0));
}

#[test]
fn gfx_without_a_decoder_falls_back_to_v8() {
    // 没有解码器：AVC 档全滤掉，只剩无 AVC 的 V8 兜底。
    let versions = advertised_versions(None);
    assert_eq!(versions, vec![CapabilityVersion::V8.0]);
}

#[test]
fn gfx_without_yuv_planes_advertises_avc420_but_not_avc444() {
    // 只出 RGBA 的解码器：AVC420（8.1）可报，AVC444（10.x）不能报。
    let versions = advertised_versions(Some(Box::new(FakeDecoder { yuv: false })));
    assert!(versions.contains(&CapabilityVersion::V8_1.0));
    assert!(!versions.contains(&CapabilityVersion::V10_7.0));
    assert!(!versions.contains(&CapabilityVersion::V10.0));
}