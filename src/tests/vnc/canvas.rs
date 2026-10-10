    use super::*;

    /// 整屏搬运与快照的 CPU 量尺，不包含网络、解码和 GPU 上传。
    /// macOS arm64 release：1080p 4.051 → 0.713 ms，4K 15.777 → 2.900 ms；
    /// 优化后包含不透明 alpha 修正，详情见 docs/remote-desktop-rendering.md。
    #[test]
    #[ignore = "性能量尺，按需手动跑"]
    fn frame_copy_cost() {
        for (width, height) in [(1920, 1080), (3840, 2160)] {
            let mut canvas = Canvas::new();
            canvas.resize(width, height);
            let pixels = vec![255; width as usize * height as usize * 4];
            let area = rect(0, 0, width, height);
            canvas.apply_raw(&area, &pixels);
            const FRAMES: u32 = 120;
            let start = std::time::Instant::now();
            for _ in 0..FRAMES {
                canvas.apply_raw(&area, std::hint::black_box(&pixels));
                std::hint::black_box(canvas.to_render_image());
            }
            println!(
                "VNC {width}×{height} apply + snapshot: {:?}/frame",
                start.elapsed() / FRAMES
            );
        }
    }

    fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// 一格 BGRA。
    fn pixel(b: u8, g: u8, r: u8) -> [u8; 4] {
        [b, g, r, 255]
    }

    #[test]
    fn raw_pixels_land_where_they_are_told() {
        let mut canvas = Canvas::new();
        canvas.resize(2, 2);

        let block: Vec<u8> = [
            pixel(1, 0, 0),
            pixel(2, 0, 0),
            pixel(3, 0, 0),
            pixel(4, 0, 0),
        ]
        .concat();
        canvas.apply_raw(&rect(0, 0, 2, 2), &block);

        assert_eq!(&canvas.pixels[0..4], &pixel(1, 0, 0));
        assert_eq!(&canvas.pixels[12..16], &pixel(4, 0, 0));
    }

    /// 越界的矩形只画能落地的部分，不 panic —— 改尺寸那一批更新里，
    /// 按旧尺寸发的增量可能还在路上，这是常态不是异常。
    #[test]
    fn rectangles_over_running_edges_are_clipped() {
        let mut canvas = Canvas::new();
        canvas.resize(1, 1);
        canvas.apply_raw(&rect(0, 0, 64, 64), &[0; 8]);
        canvas.apply_raw(&rect(5, 5, 2, 2), &[0; 16]);
        canvas.apply_copy(&rect(0, 0, 4, 4), &rect(0, 0, 4, 4));
        canvas.apply_copy(&rect(3, 3, 1, 1), &rect(0, 0, 1, 1));
    }

    /// 短了的 `data` 逐像素落位：到位的字节各归各的格子，差最后一格的
    /// 保持旧值 —— 行距按线上矩形算，半行的字节不会糊到别行去。
    #[test]
    fn a_truncated_payload_stops_at_the_last_whole_pixel() {
        let mut canvas = Canvas::new();
        canvas.resize(2, 2);
        let partial: Vec<u8> = [pixel(1, 0, 0), pixel(2, 0, 0), pixel(3, 0, 0)].concat();
        canvas.apply_raw(&rect(0, 0, 2, 2), &partial);
        assert_eq!(&canvas.pixels[4..8], &pixel(2, 0, 0));
        assert_eq!(
            &canvas.pixels[8..12],
            &pixel(3, 0, 0),
            "第三格落在它自己的格子"
        );
        assert_eq!(&canvas.pixels[12..16], &[0, 0, 0, 0], "缺的字节不发明像素");
    }

    /// 高彩一档：RGB565 的两个字节展开成 BGRA，两头精确。
    #[test]
    fn high_color_expands_to_full_range_bgra() {
        let mut canvas = Canvas::with_depth(VncColorDepth::High16);
        canvas.resize(2, 1);
        // 纯红 (31,0,0)、纯白 (63 全 1)。大端：高位字节在前。
        let wire: Vec<u8> = vec![0xF8, 0x00, 0xFF, 0xFF];
        canvas.apply_raw(&rect(0, 0, 2, 1), &wire);
        assert_eq!(&canvas.pixels[0..4], &[0, 0, 255, 255]);
        assert_eq!(&canvas.pixels[4..8], &[255, 255, 255, 255]);
    }

    /// 256 色一档：RGB332 单字节展开。
    #[test]
    fn indexed_color_expands_to_full_range_bgra() {
        let mut canvas = Canvas::with_depth(VncColorDepth::Indexed8);
        canvas.resize(2, 1);
        let wire = vec![0xE0, 0xFF]; // 纯红与纯白
        canvas.apply_raw(&rect(0, 0, 2, 1), &wire);
        assert_eq!(&canvas.pixels[0..4], &[0, 0, 255, 255]);
        assert_eq!(&canvas.pixels[4..8], &[255, 255, 255, 255]);
    }

    #[test]
    fn copies_survive_overlap() {
        // 一行四格，把左边两格往右挪一格：重叠区必须 memmove 语义，
        // 挪完 1,2,1,2 而不是 1,2,2,2。
        let mut canvas = Canvas::new();
        canvas.resize(4, 1);
        let block: Vec<u8> = [
            pixel(1, 0, 0),
            pixel(2, 0, 0),
            pixel(0, 0, 0),
            pixel(0, 0, 0),
        ]
        .concat();
        canvas.apply_raw(&rect(0, 0, 4, 1), &block);
        canvas.apply_copy(&rect(1, 0, 2, 1), &rect(0, 0, 2, 1));

        assert_eq!(&canvas.pixels[4..8], &pixel(1, 0, 0));
        assert_eq!(&canvas.pixels[8..12], &pixel(2, 0, 0));
    }

    /// 整屏往下滚一行（目的在下面、与源重叠）：从下往上搬才对。
    /// 按行顺序搬会把刚盖上去的新行又往下抄一遍，整屏变成一个内容 ——
    /// 那是"每行都画第一行"的经典错。
    #[test]
    fn a_downward_scroll_keeps_every_line() {
        let mut canvas = Canvas::new();
        canvas.resize(1, 3);
        let block: Vec<u8> = [pixel(1, 0, 0), pixel(2, 0, 0), pixel(3, 0, 0)].concat();
        canvas.apply_raw(&rect(0, 0, 1, 3), &block);
        // 源是第 0、1 行，搬到第 1、2 行：滚一行。
        canvas.apply_copy(&rect(0, 1, 1, 2), &rect(0, 0, 1, 2));

        // 往上挪一格才轮得到"最后一行还在原地"；这里第 2 行接的是第 1 行的
        // 内容，第 0 行只是恰好没当过目标。按行顺序搬的话第 2 行会错拿成
        // 已被覆盖的第 1 行 —— 三条断言里钉的就是这个差别。
        assert_eq!(&canvas.pixels[0..4], &pixel(1, 0, 0));
        assert_eq!(&canvas.pixels[4..8], &pixel(1, 0, 0));
        assert_eq!(
            &canvas.pixels[8..12],
            &pixel(2, 0, 0),
            "第 2 行该是原第 1 行"
        );
    }

    #[test]
    fn resize_clears_and_reuses_when_unchanged() {
        let mut canvas = Canvas::new();
        canvas.resize(2, 1);
        canvas.apply_raw(&rect(0, 0, 2, 1), &pixel(9, 9, 9).repeat(2));
        canvas.resize(2, 1);
        assert_eq!(&canvas.pixels[0..4], &pixel(9, 9, 9), "同尺寸是重复公告");
        canvas.resize(1, 1);
        assert_eq!(&canvas.pixels, &[0, 0, 0, 0], "换尺寸清零等新帧");
    }

    #[test]
    fn no_image_before_the_first_resolution() {
        let canvas = Canvas::new();
        assert!(canvas.to_render_image().is_none());
    }

    /// "画过没有"这本账：空画布一个像素都没画过，画满了才算铺满，
    /// 换尺寸清零之后从头再来。会话按它决定要增量还是要全量。
    #[test]
    fn a_canvas_knows_which_pixels_have_never_been_painted() {
        let mut canvas = Canvas::new();
        assert!(!canvas.fully_painted(), "还没定下尺寸，谈不上铺满");

        canvas.resize(2, 2);
        assert_eq!(canvas.unpainted(), 4);
        assert!(!canvas.fully_painted());

        // 画左边一列：还剩右边一列没画过。
        canvas.apply_raw(&rect(0, 0, 1, 2), &pixel(1, 0, 0).repeat(2));
        assert_eq!(canvas.unpainted(), 2);

        canvas.apply_raw(&rect(1, 0, 1, 2), &pixel(2, 0, 0).repeat(2));
        assert!(canvas.fully_painted(), "四格都画过了");

        // 同一块再画一遍不会把账记成负数。
        canvas.apply_raw(&rect(0, 0, 2, 2), &pixel(3, 0, 0).repeat(4));
        assert_eq!(canvas.unpainted(), 0);

        // 换尺寸 = 整块作废，全部重新算没画过。
        canvas.resize(3, 1);
        assert_eq!(canvas.unpainted(), 3);
        assert!(!canvas.fully_painted());
    }

    /// 正文短了一截时，没落地的那几格得继续算"没画过" —— 它们在画布上
    /// 是黑的，而增量请求永远要不回来（那一片对服务器来说没"变过"）。
    #[test]
    fn a_truncated_payload_leaves_the_rest_unpainted() {
        let mut canvas = Canvas::new();
        canvas.resize(2, 2);
        let partial: Vec<u8> = [pixel(1, 0, 0), pixel(2, 0, 0), pixel(3, 0, 0)].concat();
        canvas.apply_raw(&rect(0, 0, 2, 2), &partial);
        assert_eq!(canvas.unpainted(), 1, "缺的那一格还欠着");
    }

    /// CopyRect 把"画过没有"一起搬：从没画过的地方搬来的是一块黑，
    /// 目的地就该继续算没画过。否则一次滚屏就能把黑块洗成"已经画好的
    /// 黑"，那块洞再也要不回来。
    #[test]
    fn a_copy_carries_the_unpainted_flag_with_the_pixels() {
        let mut canvas = Canvas::new();
        canvas.resize(2, 1);
        // 只画右边一格，左边那格从没画过。
        canvas.apply_raw(&rect(1, 0, 1, 1), &pixel(7, 0, 0));
        assert_eq!(canvas.unpainted(), 1);

        // 把左边那格（没画过）搬到右边：右边也回到"没画过"。
        canvas.apply_copy(&rect(1, 0, 1, 1), &rect(0, 0, 1, 1));
        assert_eq!(canvas.unpainted(), 2, "搬来一块黑，不算把它画上了");

        // 反过来：画好的一格搬到没画过的那格，账才该减。
        canvas.apply_raw(&rect(0, 0, 1, 1), &pixel(8, 0, 0));
        canvas.apply_copy(&rect(1, 0, 1, 1), &rect(0, 0, 1, 1));
        assert_eq!(canvas.unpainted(), 0);
    }

    /// 手动"刷新画面"：画面不动，但账整个作废 —— 下一拍会去要全量。
    #[test]
    fn forgetting_the_paint_keeps_the_pixels_but_asks_again() {
        let mut canvas = Canvas::new();
        canvas.resize(1, 1);
        canvas.apply_raw(&rect(0, 0, 1, 1), &pixel(5, 6, 7));
        assert!(canvas.fully_painted());

        canvas.forget_paint();
        assert!(!canvas.fully_painted());
        assert_eq!(&canvas.pixels[0..4], &pixel(5, 6, 7), "像素一个都不动");
    }
