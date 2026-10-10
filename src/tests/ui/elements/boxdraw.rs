    use super::*;

    fn cell() -> Bounds<Pixels> {
        Bounds::new(point(px(10.), px(20.)), size(px(9.), px(21.)))
    }

    fn extents(ink: &[Ink]) -> (f32, f32, f32, f32) {
        let (mut nx, mut xx, mut ny, mut xy) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        let mut visit = |x: f32, y: f32| {
            nx = nx.min(x);
            xx = xx.max(x);
            ny = ny.min(y);
            xy = xy.max(y);
        };
        for i in ink {
            match i {
                Ink::Rect(b) | Ink::Shade(b, _) => {
                    let (x, y) = (b.origin.x.as_f32(), b.origin.y.as_f32());
                    visit(x, y);
                    visit(x + b.size.width.as_f32(), y + b.size.height.as_f32());
                }
                Ink::Path(p) => {
                    for v in &p.vertices {
                        visit(v.xy_position.x.as_f32(), v.xy_position.y.as_f32());
                    }
                }
            }
        }
        (nx, xx, ny, xy)
    }

    #[test]
    fn the_whole_range_is_covered() {
        for cp in 0x2500u32..=0x259f {
            let c = char::from_u32(cp).unwrap();
            assert!(
                glyph(c, cell(), 1.).is_some(),
                "U+{cp:04X} {c} fell through to the font"
            );
        }
    }

    #[test]
    fn ink_stays_inside_the_cell() {
        let b = cell();
        let (x0, y0) = (b.origin.x.as_f32(), b.origin.y.as_f32());
        let (x1, y1) = (x0 + b.size.width.as_f32(), y0 + b.size.height.as_f32());
        for cp in 0x2500u32..=0x259f {
            let c = char::from_u32(cp).unwrap();
            let (nx, xx, ny, xy) = extents(&glyph(c, b, 1.).unwrap());
            assert!(
                nx >= x0 - 0.5 && xx <= x1 + 0.5 && ny >= y0 - 0.5 && xy <= y1 + 0.5,
                "U+{cp:04X} {c} paints outside the cell: \
                 x {nx}..{xx} vs {x0}..{x1}, y {ny}..{xy} vs {y0}..{y1}"
            );
        }
    }

    #[test]
    fn arms_reach_their_edges() {
        let b = cell();
        let (x0, y0) = (b.origin.x.as_f32(), b.origin.y.as_f32());
        let (x1, y1) = (x0 + b.size.width.as_f32(), y0 + b.size.height.as_f32());
        for cp in 0x2500u32..=0x259f {
            let c = char::from_u32(cp).unwrap();
            let Some((u, d, l, r)) = arms_of(c) else {
                continue;
            };
            let (nx, xx, ny, xy) = extents(&glyph(c, b, 1.).unwrap());
            if u != Arm::None {
                assert_eq!(ny, y0, "{c}: up arm misses the top edge");
            }
            if d != Arm::None {
                assert_eq!(xy, y1, "{c}: down arm misses the bottom edge");
            }
            if l != Arm::None {
                assert_eq!(nx, x0, "{c}: left arm misses the left edge");
            }
            if r != Arm::None {
                assert_eq!(xx, x1, "{c}: right arm misses the right edge");
            }
        }
    }

    #[test]
    fn doubles_rounded_and_diagonals_reach_their_edges() {
        let b = cell();
        let (x0, y0) = (b.origin.x.as_f32(), b.origin.y.as_f32());
        let (x1, y1) = (x0 + b.size.width.as_f32(), y0 + b.size.height.as_f32());
        let expect = [
            ('═', false, false, true, true),
            ('║', true, true, false, false),
            ('╔', false, true, false, true),
            ('╬', true, true, true, true),
            ('╠', true, true, false, true),
            ('╦', false, true, true, true),
            ('╭', false, true, false, true),
            ('╮', false, true, true, false),
            ('╯', true, false, true, false),
            ('╰', true, false, false, true),
            ('╱', true, true, true, true),
            ('╲', true, true, true, true),
        ];
        for (c, u, d, l, r) in expect {
            let (nx, xx, ny, xy) = extents(&glyph(c, b, 1.).unwrap());
            if u {
                assert_eq!(ny, y0, "{c}: misses the top edge");
            }
            if d {
                assert_eq!(xy, y1, "{c}: misses the bottom edge");
            }
            if l {
                assert_eq!(nx, x0, "{c}: misses the left edge");
            }
            if r {
                assert_eq!(xx, x1, "{c}: misses the right edge");
            }
        }
    }

    #[test]
    fn double_cross_keeps_its_open_centre() {
        let b = cell();
        let cx = b.origin.x.as_f32() + b.size.width.as_f32() / 2.;
        let cy = b.origin.y.as_f32() + b.size.height.as_f32() / 2.;
        for i in glyph('╬', b, 1.).unwrap() {
            let Ink::Rect(r) = i else {
                panic!("╬ should be rects only");
            };
            let (x, y) = (r.origin.x.as_f32(), r.origin.y.as_f32());
            let inside = cx > x
                && cx < x + r.size.width.as_f32()
                && cy > y
                && cy < y + r.size.height.as_f32();
            assert!(!inside, "╬'s centre is covered");
        }
    }

    #[test]
    fn blocks_cover_their_nominal_area() {
        let b = cell();
        let (x0, y0) = (b.origin.x.as_f32(), b.origin.y.as_f32());
        let (w, h) = (b.size.width.as_f32(), b.size.height.as_f32());
        let (nx, xx, ny, xy) = extents(&glyph('█', b, 1.).unwrap());
        assert_eq!(
            (nx, xx, ny, xy),
            (x0, x0 + w, y0, y0 + h),
            "█ isn't the full cell"
        );
        let (_, _, ny, xy) = extents(&glyph('▀', b, 1.).unwrap());
        assert_eq!(ny, y0, "▀ doesn't reach the top");
        assert!((xy - (y0 + h / 2.)).abs() <= 0.5, "▀ isn't the top half");
        let (_, _, ny, xy) = extents(&glyph('▄', b, 1.).unwrap());
        assert_eq!(xy, y0 + h, "▄ doesn't reach the bottom");
        assert!((ny - (y0 + h / 2.)).abs() <= 0.5, "▄ isn't the bottom half");
        for (c, alpha) in [('░', 0.25), ('▒', 0.5), ('▓', 0.75)] {
            let ink = glyph(c, b, 1.).unwrap();
            assert_eq!(ink.len(), 1);
            let Ink::Shade(r, a) = &ink[0] else {
                panic!("{c} should be a shade");
            };
            assert_eq!(*a, alpha);
            assert_eq!(r.size.width.as_f32(), w, "{c} doesn't wash the full cell");
        }
    }

    #[test]
    fn stroke_weights_are_ordered_and_visible() {
        let light = {
            let Ink::Rect(r) = &glyph('│', cell(), 1.).unwrap()[0] else {
                panic!()
            };
            r.size.width.as_f32()
        };
        let heavy = {
            let Ink::Rect(r) = &glyph('┃', cell(), 1.).unwrap()[0] else {
                panic!()
            };
            r.size.width.as_f32()
        };
        assert!(light >= 1., "light stroke thinner than a pixel");
        assert!(heavy > light, "heavy stroke isn't heavier");
        let tiny = Bounds::new(point(px(0.), px(0.)), size(px(2.), px(4.)));
        let Ink::Rect(r) = &glyph('│', tiny, 1.).unwrap()[0] else {
            panic!()
        };
        assert!(r.size.width.as_f32() >= 1.);
    }

    #[test]
    fn straight_strokes_snap_to_device_pixels() {
        let scale = 2.0;
        let b = Bounds::new(point(px(10.37), px(20.11)), size(px(9.03), px(21.)));
        let on_grid = |v: f32| ((v * scale).round() - v * scale).abs() < 1e-3;
        for cp in 0x2500u32..=0x259f {
            let c = char::from_u32(cp).unwrap();
            for i in glyph(c, b, scale).unwrap() {
                let (Ink::Rect(r) | Ink::Shade(r, _)) = i else {
                    continue;
                };
                let (x, y) = (r.origin.x.as_f32(), r.origin.y.as_f32());
                let (x2, y2) = (x + r.size.width.as_f32(), y + r.size.height.as_f32());
                assert!(
                    on_grid(x) && on_grid(y) && on_grid(x2) && on_grid(y2),
                    "U+{cp:04X} {c}: stroke edge off the device grid \
                     ({x}, {y})..({x2}, {y2}) at scale {scale}"
                );
            }
        }
        let below = Bounds::new(point(px(10.37), px(41.11)), size(px(9.03), px(21.)));
        let bottom = extents(&glyph('│', b, scale).unwrap()).3;
        let top = extents(&glyph('│', below, scale).unwrap()).2;
        assert_eq!(bottom, top, "adjacent │ cells no longer tile");
    }

    #[test]
    fn stroke_weight_is_uniform_across_cells_at_any_scale() {
        for (cw, lh) in [(7.8f32, 18.0f32), (9.03, 21.0), (9.6, 22.0), (10.8, 25.0)] {
            for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0] {
                let widths: Vec<f32> = (0..24)
                    .map(|i| {
                        let b = Bounds::new(point(px(cw * i as f32), px(0.)), size(px(cw), px(lh)));
                        let Ink::Rect(r) = &glyph('│', b, scale).unwrap()[0] else {
                            panic!("│ should be a rect")
                        };
                        (r.size.width.as_f32() * scale).round()
                    })
                    .collect();
                let (lo, hi) = (
                    widths.iter().cloned().fold(f32::MAX, f32::min),
                    widths.iter().cloned().fold(f32::MIN, f32::max),
                );
                assert_eq!(
                    lo, hi,
                    "│ weight varies {lo}..{hi} device px across columns \
                     (cell_width {cw}, scale {scale}): {widths:?}"
                );
                assert!(lo >= 1., "│ thinner than a device pixel at scale {scale}");

                let heights: Vec<f32> = (0..24)
                    .map(|r| {
                        let b = Bounds::new(point(px(0.), px(lh * r as f32)), size(px(cw), px(lh)));
                        let Ink::Rect(rect) = &glyph('─', b, scale).unwrap()[0] else {
                            panic!("─ should be a rect")
                        };
                        (rect.size.height.as_f32() * scale).round()
                    })
                    .collect();
                let (lo, hi) = (
                    heights.iter().cloned().fold(f32::MAX, f32::min),
                    heights.iter().cloned().fold(f32::MIN, f32::max),
                );
                assert_eq!(
                    lo, hi,
                    "─ weight varies {lo}..{hi} device px across rows \
                     (cell_width {cw}, scale {scale}): {heights:?}"
                );
            }
        }
    }

    #[test]
    fn integer_scales_keep_their_previous_thickness() {
        for (cw, lh) in [(7.8f32, 18.0f32), (9.03, 21.0), (9.6, 22.0), (10.8, 25.0)] {
            for scale in [1.0f32, 2.0, 3.0] {
                let previous = (cw * 0.15).round().max(1.);
                assert_eq!(
                    light_thickness(cw, scale),
                    previous,
                    "cell_width {cw} at scale {scale} changed weight"
                );
                let b = Bounds::new(point(px(0.), px(0.)), size(px(cw), px(lh)));
                let Ink::Rect(l) = &glyph('│', b, scale).unwrap()[0] else {
                    panic!()
                };
                let Ink::Rect(h) = &glyph('┃', b, scale).unwrap()[0] else {
                    panic!()
                };
                assert!(h.size.width.as_f32() > l.size.width.as_f32());
            }
        }
    }

    #[test]
    fn powerline_separators_fill_the_cell() {
        let b = cell();
        let (x0, y0) = (b.origin.x.as_f32(), b.origin.y.as_f32());
        let (x1, y1) = (x0 + b.size.width.as_f32(), y0 + b.size.height.as_f32());
        for c in [
            '\u{e0b0}', '\u{e0b2}', '\u{e0b4}', '\u{e0b6}', '\u{e0b8}', '\u{e0ba}', '\u{e0bc}',
            '\u{e0be}',
        ] {
            let ink = powerline(c, b, 1., 1.).expect("separator should be drawn natively");
            let (nx, xx, ny, xy) = extents(&ink);
            assert!(
                nx >= x0 - 0.5 && xx <= x1 + 0.5 && ny >= y0 - 0.5 && xy <= y1 + 0.5,
                "U+{:04X} paints outside the cell",
                c as u32
            );
            assert!((ny - y0).abs() <= 0.5, "U+{:04X} misses the top", c as u32);
            assert!(
                (xy - y1).abs() <= 0.5,
                "U+{:04X} misses the bottom",
                c as u32
            );
        }
        assert!(
            powerline('\u{e0b1}', b, 1., 1.).is_none(),
            "thin chevron is a font glyph"
        );
        assert!(powerline('a', b, 1., 1.).is_none());
    }

    #[test]
    fn dim_powerline_skips_the_seam_cover() {
        // 半透明时补边会和路径叠加，那一列反而更深 —— 只在不透明时补。
        let b = Bounds::new(point(px(10.4), px(0.)), size(px(9.), px(21.)));
        let opaque = powerline('\u{e0b0}', b, 2., 1.).unwrap();
        let dim = powerline('\u{e0b0}', b, 2., 0.7).unwrap();
        assert_eq!(opaque.len(), 2, "opaque separator should cover its seam");
        assert_eq!(dim.len(), 1, "dim separator should be the path alone");
    }

    #[test]
    fn ink_routes_both_ranges_and_passes_everything_else_through() {
        let b = cell();
        assert!(ink('│', b, 1., 1.).is_some());
        assert!(ink('█', b, 1., 1.).is_some());
        assert!(ink('\u{e0b0}', b, 1., 1.).is_some());
        // 盲文（btop 的折线图）与普通文字仍旧走字体。
        assert!(ink('⣿', b, 1., 1.).is_none());
        assert!(ink('A', b, 1., 1.).is_none());
    }
