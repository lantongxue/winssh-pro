use super::*;
use ironrdp_graphics::image_processing::PixelFormat;
use winssh_desktop::DesktopFrame;

fn rect(left: u16, top: u16, right: u16, bottom: u16) -> InclusiveRectangle {
    InclusiveRectangle {
        left,
        top,
        right,
        bottom,
    }
}

fn tiled(frame: &DesktopFrame) -> &winssh_desktop::TiledFrame {
    let DesktopFrame::Tiled(tiled) = frame else {
        panic!("expected a tiled frame")
    };
    tiled
}

#[test]
fn grid_covers_non_multiple_sizes_with_smaller_edge_tiles() {
    assert_eq!(grid(256, 256), (1, 1));
    assert_eq!(grid(257, 256), (2, 1));
    assert_eq!(grid(1920, 1080), (8, 5));
    assert_eq!(tile_rect(300, 300, 0, 0), (0, 0, 256, 256));
    assert_eq!(tile_rect(300, 300, 1, 1), (256, 256, 44, 44));
}

#[test]
fn dirty_rects_mark_exactly_the_tiles_they_touch() {
    // 跨在四格交点上的一条小矩形，四格都脏。
    let flags = dirty_grid(512, 512, &[rect(255, 255, 256, 256)]);
    assert_eq!(flags, vec![true, true, true, true]);
    let flags = dirty_grid(512, 512, &[rect(0, 0, 10, 10)]);
    assert_eq!(flags, vec![true, false, false, false]);
    // 越界矩形裁到桌面内；空矩形（left > right）跳过。
    let flags = dirty_grid(300, 300, &[rect(290, 290, 60000, 60000)]);
    assert_eq!(flags, vec![false, false, false, true]);
    let flags = dirty_grid(300, 300, &[rect(5, 5, 2, 2)]);
    assert_eq!(flags, vec![false; 4]);
}

#[test]
fn first_build_creates_every_tile_with_opaque_alpha() {
    let image = DecodedImage::new(PixelFormat::BgrX32, 300, 300);
    let mut cache = TileCache::default();
    let frame = cache.build(&image, &[]).expect("frame");
    let tiled = tiled(&frame);
    assert_eq!((tiled.cols(), tiled.rows()), (2, 2));
    assert_eq!(tiled.tile_size(1, 1), (44, 44));
    // 画布全零（X=0），每格都必须补成不透明，不能当透明位图上传。
    for tile in tiled.tiles() {
        assert!(
            tile.as_bytes(0)
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel == &[0, 0, 0, 255])
        );
    }
}

#[test]
fn only_dirty_tiles_are_rebuilt_and_clean_tiles_keep_their_identity() {
    let image = DecodedImage::new(PixelFormat::BgrX32, 512, 512);
    let mut cache = TileCache::default();
    let first = cache.build(&image, &[]).unwrap();
    let second = cache.build(&image, &[rect(0, 0, 10, 10)]).unwrap();
    let (a, b) = (tiled(&first), tiled(&second));
    assert!(!Arc::ptr_eq(a.tile(0, 0), b.tile(0, 0)));
    for (col, row) in [(1, 0), (0, 1), (1, 1)] {
        assert!(Arc::ptr_eq(a.tile(col, row), b.tile(col, row)));
    }
    // 没有脏矩形：全部复用，不新建任何图。
    let third = cache.build(&image, &[]).unwrap();
    let c = tiled(&third);
    for index in 0..4 {
        assert!(Arc::ptr_eq(&b.tiles()[index], &c.tiles()[index]));
    }
}

#[test]
fn resize_and_invalidate_force_a_full_rebuild() {
    let mut cache = TileCache::default();
    let small = DecodedImage::new(PixelFormat::BgrX32, 256, 256);
    let first = cache.build(&small, &[]).unwrap();
    let big = DecodedImage::new(PixelFormat::BgrX32, 512, 512);
    // 尺寸变了：即使只报了一小块损伤，也要整屏重建。
    let second = cache.build(&big, &[rect(0, 0, 1, 1)]).unwrap();
    let (a, b) = (tiled(&first), tiled(&second));
    assert_eq!(b.tiles().len(), 4);
    assert!(!Arc::ptr_eq(a.tile(0, 0), b.tile(0, 0)));
    // 重新激活后画布清零，invalidate 同样全量重建。
    cache.invalidate();
    let third = cache.build(&big, &[rect(0, 0, 1, 1)]).unwrap();
    let c = tiled(&third);
    for index in 0..4 {
        assert!(!Arc::ptr_eq(&b.tiles()[index], &c.tiles()[index]));
    }
}

#[test]
fn zero_sized_canvas_publishes_nothing() {
    let image = DecodedImage::new(PixelFormat::BgrX32, 0, 0);
    assert!(TileCache::default().build(&image, &[]).is_none());
}