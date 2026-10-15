use super::*;

#[test]
fn tray_icon_rgba_has_expected_buffer_length() {
    let rgba = icon_rgba();
    let expected_len = (ICON_SIZE * ICON_SIZE * 4) as usize;
    assert_eq!(
        rgba.len(),
        expected_len,
        "托盘图标的 RGBA 字节流长度应当严格等于 ICON_SIZE * ICON_SIZE * 4"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_tray_icon_conforms_to_apple_template_specification() {
    let decoded = image::load_from_memory(crate::assets::TRAY_ICON_MACOS_PNG)
        .expect("macOS 托盘模板图标 PNG 应当能够正常解码");
    assert_eq!(decoded.width(), 44, "macOS 托盘图标宽度应当为 44px (22pt @2x)");
    assert_eq!(decoded.height(), 44, "macOS 托盘图标高度应当为 44px (22pt @2x)");

    let rgba = decoded.to_rgba8();
    let mut has_transparent = false;
    let mut has_visible = false;

    for pixel in rgba.pixels() {
        let [r, g, b, a] = pixel.0;
        if a == 0 {
            has_transparent = true;
        } else {
            has_visible = true;
            // Apple AppKit 的 template 图像规范要求 RGB 通道为纯单色（标准为纯黑 (0,0,0)）
            // 依赖 Alpha 通道定义轮廓与平滑抗锯齿，以便 AppKit 自主映射系统深浅色与选中态高亮。
            assert_eq!(
                (r, g, b),
                (0, 0, 0),
                "非完全透明像素的 RGB 通道必须为纯黑以保证模板着色正常：({r}, {g}, {b}, {a})"
            );
        }
    }

    assert!(has_transparent, "macOS 托盘图标必须具有透明背景");
    assert!(has_visible, "macOS 托盘图标必须包含可见的前景图标内容");
}
