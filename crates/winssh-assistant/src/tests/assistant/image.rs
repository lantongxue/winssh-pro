    use super::*;

    /// 一张指定尺寸的图，按格式编好。内容是一道渐变，免得全色块被压得过分小。
    fn encoded(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    /// 剪贴板截图那一档：一张超宽的 BMP 进来，出去的是长边 1568 的 PNG，
    /// 宽高比不变。
    #[test]
    fn oversized_bitmaps_shrink_into_png() {
        let prepared = ChatImage::prepare(&encoded(3136, 200, image::ImageFormat::Bmp)).unwrap();
        assert!(prepared.is_png(), "BMP 该转成 PNG：{prepared:?}");
        assert_eq!(prepared.width(), MAX_EDGE);
        assert_eq!(prepared.height(), 100, "宽高比该保持");
        let decoded = image::load_from_memory(prepared.bytes()).unwrap();
        assert_eq!(
            decoded.dimensions(),
            (MAX_EDGE, 100),
            "字节里的尺寸该和记下的一致"
        );
    }

    /// 尺寸合适的 PNG / JPEG 一个字节都不动。
    #[test]
    fn fitting_png_and_jpeg_pass_through() {
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let bytes = encoded(320, 200, format);
            let prepared = ChatImage::prepare(&bytes).unwrap();
            assert_eq!(
                prepared.bytes(),
                bytes.as_slice(),
                "{format:?} 不该重新编码"
            );
            assert_eq!((prepared.width(), prepared.height()), (320, 200));
        }
        // 模型不一定收的格式（这里是 GIF）即使尺寸合适也要转。
        let prepared = ChatImage::prepare(&encoded(32, 32, image::ImageFormat::Gif)).unwrap();
        assert!(prepared.is_png());
    }

    /// PNG 超过上限时退成 JPEG。用一张噪点图：PNG 压不动它，JPEG 压得动。
    #[test]
    fn noisy_images_fall_back_to_jpeg() {
        let mut seed = 0x2545_f491_u32;
        let noise = image::RgbImage::from_fn(96, 96, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let [r, g, b, _] = seed.to_le_bytes();
            image::Rgb([r, g, b])
        });
        let mut bmp = Vec::new();
        DynamicImage::ImageRgb8(noise)
            .write_to(&mut Cursor::new(&mut bmp), image::ImageFormat::Bmp)
            .unwrap();

        let png = ChatImage::prepare(&bmp).unwrap();
        assert!(png.is_png());
        let jpeg = ChatImage::prepare_within(&bmp, png.bytes().len() - 1).unwrap();
        assert!(!jpeg.is_png(), "PNG 超了上限该退成 JPEG");
        assert_eq!(jpeg.media_type(), "image/jpeg");
        assert!(jpeg.bytes().len() < png.bytes().len());
    }

    /// alpha 全 0 的图（Windows 剪贴板 32 位 DIB 常见的样子）当不透明的读，
    /// 否则模型看到的是一块黑。有一个像素不透明就说明 alpha 是真的，不动。
    #[test]
    fn blank_alpha_is_treated_as_opaque() {
        let blank = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 100, 50, 0]));
        let fixed = opaque_if_blank_alpha(DynamicImage::ImageRgba8(blank));
        assert!(!fixed.color().has_alpha());
        assert_eq!(fixed.to_rgb8().get_pixel(0, 0).0, [200, 100, 50]);

        let mut real = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]));
        real.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        assert!(
            opaque_if_blank_alpha(DynamicImage::ImageRgba8(real))
                .color()
                .has_alpha()
        );
    }

    /// 认不出、解不开的字节要报错，不能 panic。
    #[test]
    fn garbage_is_an_error() {
        assert!(ChatImage::prepare(b"definitely not an image").is_err());
        let mut truncated = encoded(64, 64, image::ImageFormat::Png);
        truncated.truncate(40);
        assert!(ChatImage::prepare(&truncated).is_err());
    }

    /// 库里读回来的只认 PNG / JPEG。
    #[test]
    fn stored_images_only_come_back_as_png_or_jpeg() {
        let png = encoded(8, 8, image::ImageFormat::Png);
        let image = ChatImage::from_stored("image/png", png.clone(), 8, 8).unwrap();
        assert_eq!(image.bytes(), png.as_slice());
        assert!(ChatImage::from_stored("image/gif", png.clone(), 8, 8).is_none());
        assert!(ChatImage::from_stored("image/png", Vec::new(), 8, 8).is_none());
    }

    #[test]
    fn image_paths_are_recognized_by_extension() {
        assert!(looks_like_image(Path::new("C:/shots/error.PNG")));
        assert!(looks_like_image(Path::new("/tmp/a.jpeg")));
        assert!(!looks_like_image(Path::new("/tmp/notes.txt")));
        assert!(!looks_like_image(Path::new("/tmp/noext")));
    }
