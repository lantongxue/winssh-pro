//! Local patch (see PATCHES.md): native cursors for [`gpui::CursorImage`].
//!
//! The image goes to Windows in the form the application describes it — per
//! pixel alpha, a classic AND/XOR cursor whose inversion the system performs,
//! or a plain monochrome one — through `CreateIconIndirect`. The cursor is shown
//! at the image's pixel size; Windows doesn't scale cursors made this way.

use std::cell::RefCell;
use std::sync::{Arc, Weak};

use anyhow::{Result, anyhow};
use gpui::{CursorImage, CursorImageId, CursorPixels};
use gpui_util::ResultExt;
use windows::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::*};

/// Native cursors made for [`CursorImage`]s, keyed by image id.
///
/// A cursor lives as long as the application holds its image. Once the image
/// is gone it is destroyed — but only when no window still uses the handle:
/// windows learn about cursor changes through posted messages, so the
/// platform having moved on doesn't mean every window has.
#[derive(Default)]
pub(crate) struct CursorImageCache {
    entries: RefCell<Vec<CachedCursor>>,
}

struct CachedCursor {
    id: CursorImageId,
    image: Weak<CursorImage>,
    native: NativeCursor,
}

/// What a [`CursorImage`] became on this platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeCursor {
    /// The pointer is hidden.
    Hidden,
    Shown(HCURSOR),
    /// Windows refused to make it (logged once); the caller falls back to a style.
    Unavailable,
}

impl CursorImageCache {
    /// The native cursor for `image`, made on first use.
    pub(crate) fn cursor(&self, image: &Arc<CursorImage>) -> NativeCursor {
        let mut entries = self.entries.borrow_mut();
        if let Some(entry) = entries.iter().find(|entry| entry.id == image.id()) {
            return entry.native;
        }
        let native = match create_cursor(image) {
            Ok(Some(cursor)) => NativeCursor::Shown(cursor),
            Ok(None) => NativeCursor::Hidden,
            Err(error) => {
                log::error!("failed to create a cursor from {image:?}: {error:#}");
                NativeCursor::Unavailable
            }
        };
        entries.push(CachedCursor {
            id: image.id(),
            image: Arc::downgrade(image),
            native,
        });
        native
    }

    /// Destroys the cursors whose image has been dropped, except those
    /// `in_use` reports as still shown somewhere.
    pub(crate) fn prune(&self, in_use: impl Fn(HCURSOR) -> bool) {
        self.entries.borrow_mut().retain(|entry| {
            if entry.image.strong_count() > 0 {
                return true;
            }
            match entry.native {
                NativeCursor::Shown(cursor) if in_use(cursor) => true,
                NativeCursor::Shown(cursor) => {
                    unsafe { DestroyCursor(cursor) }.log_err();
                    false
                }
                NativeCursor::Hidden | NativeCursor::Unavailable => false,
            }
        });
    }
}

fn create_cursor(image: &CursorImage) -> Result<Option<HCURSOR>> {
    let (width, height) = (image.width(), image.height());
    let (color, mask) = match image.pixels() {
        CursorPixels::Hidden => return Ok(None),
        CursorPixels::Bgra(bgra) => {
            // With alpha present Windows ignores the mask. A fully clear pixel
            // is kept black anyway, so it stays clear under AND/XOR too.
            let color = bgra
                .chunks_exact(4)
                .flat_map(|pixel| match pixel[3] {
                    0 => [0; 4],
                    _ => [pixel[0], pixel[1], pixel[2], pixel[3]],
                })
                .collect::<Vec<_>>();
            let mask = pack_rows(width, height, |index| bgra[index * 4 + 3] == 0);
            (Some(color), mask)
        }
        CursorPixels::Masked { and, xor } if is_monochrome(xor) => {
            // Monochrome: no color bitmap, the AND mask above the XOR mask.
            let mut mask = pack_rows(width, height, |index| and[index]);
            mask.extend(pack_rows(width, height, |index| xor[index * 4] != 0));
            (None, mask)
        }
        CursorPixels::Masked { and, xor } => {
            // No alpha anywhere, so Windows combines the color with the AND
            // mask: (screen AND mask) XOR color, which is what inverts.
            let color = xor
                .chunks_exact(4)
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 0])
                .collect::<Vec<_>>();
            (Some(color), pack_rows(width, height, |index| and[index]))
        }
    };

    let mask_height = match color {
        Some(_) => height,
        None => height * 2,
    };
    unsafe {
        let mask = CreateBitmap(
            width as i32,
            mask_height as i32,
            1,
            1,
            Some(mask.as_ptr().cast()),
        );
        if mask.is_invalid() {
            return Err(anyhow!(
                "CreateBitmap failed for a {width}×{height} cursor mask"
            ));
        }
        let color = match color {
            Some(pixels) => match color_bitmap(width, height, &pixels) {
                Ok(bitmap) => bitmap,
                Err(error) => {
                    let _ = DeleteObject(mask.into());
                    return Err(error);
                }
            },
            None => HBITMAP::default(),
        };
        let (hotspot_x, hotspot_y) = image.hotspot();
        let info = ICONINFO {
            fIcon: false.into(),
            xHotspot: hotspot_x,
            yHotspot: hotspot_y,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info);
        // The cursor holds its own copies of both bitmaps.
        let _ = DeleteObject(mask.into());
        if !color.is_invalid() {
            let _ = DeleteObject(color.into());
        }
        Ok(Some(HCURSOR(icon?.0)))
    }
}

/// A top-down 32-bit DIB with an alpha channel, filled with `pixels` (BGRA).
unsafe fn color_bitmap(width: u32, height: u32, pixels: &[u8]) -> Result<HBITMAP> {
    let header = BITMAPV5HEADER {
        bV5Size: size_of::<BITMAPV5HEADER>() as u32,
        bV5Width: width as i32,
        bV5Height: -(height as i32),
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5RedMask: 0x00ff_0000,
        bV5GreenMask: 0x0000_ff00,
        bV5BlueMask: 0x0000_00ff,
        bV5AlphaMask: 0xff00_0000,
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bitmap = unsafe {
        CreateDIBSection(
            None,
            (&raw const header).cast(),
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
    }?;
    // Rows of a 32-bit DIB are already DWORD-aligned: the bytes copy as they are.
    unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len()) };
    Ok(bitmap)
}

/// Every color is pure black or pure white, so the plain monochrome form
/// carries the image exactly.
fn is_monochrome(xor: &[u8]) -> bool {
    xor.chunks_exact(4)
        .all(|pixel| matches!(pixel[..3], [0, 0, 0] | [0xff, 0xff, 0xff]))
}

/// One bit per pixel, most significant bit first, each row padded to a WORD
/// as `CreateBitmap` requires.
fn pack_rows(width: u32, height: u32, bit: impl Fn(usize) -> bool) -> Vec<u8> {
    let (width, height) = (width as usize, height as usize);
    let stride = width.div_ceil(16) * 2;
    let mut rows = vec![0u8; stride * height];
    for y in 0..height {
        for x in 0..width {
            if bit(y * width + x) {
                rows[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap_height(bitmap: HBITMAP) -> i32 {
        let mut info: BITMAP = unsafe { std::mem::zeroed() };
        let written = unsafe {
            GetObjectW(
                bitmap.into(),
                size_of::<BITMAP>() as i32,
                Some((&raw mut info).cast()),
            )
        };
        assert_ne!(written, 0);
        info.bmHeight
    }

    #[test]
    fn rows_are_packed_msb_first_and_word_aligned() {
        assert_eq!(pack_rows(3, 2, |index| index % 2 == 0), [0xa0, 0, 0x40, 0]);
        let wide = pack_rows(17, 1, |index| index == 16);
        assert_eq!(wide, [0, 0, 0x80, 0]);
    }

    #[test]
    fn each_pixel_form_becomes_the_matching_native_cursor() {
        let masked = |xor: [u8; 4]| {
            CursorImage::new(
                2,
                2,
                (1, 1),
                CursorPixels::Masked {
                    and: vec![true, false, true, false],
                    xor: xor.repeat(4),
                },
            )
            .unwrap()
        };
        let alpha =
            CursorImage::new(2, 2, (0, 0), CursorPixels::Bgra([9, 8, 7, 128].repeat(4))).unwrap();
        // Monochrome: no color bitmap, AND and XOR stacked in one mask.
        // Color: a color bitmap next to a single-height mask.
        for (image, stacked) in [
            (masked([0xff, 0xff, 0xff, 0]), true),
            (masked([0x20, 0x40, 0x60, 0]), false),
            (alpha, false),
        ] {
            let cursor = create_cursor(&image).unwrap().expect("a visible cursor");
            let mut info = ICONINFO::default();
            unsafe { GetIconInfo(HICON(cursor.0), &mut info) }.unwrap();
            assert!(!info.fIcon.as_bool());
            assert_eq!((info.xHotspot, info.yHotspot), image.hotspot());
            assert_eq!(info.hbmColor.is_invalid(), stacked);
            assert_eq!(bitmap_height(info.hbmMask), if stacked { 4 } else { 2 });
            unsafe {
                let _ = DeleteObject(info.hbmMask.into());
                if !info.hbmColor.is_invalid() {
                    let _ = DeleteObject(info.hbmColor.into());
                }
                DestroyCursor(cursor).unwrap();
            }
        }
        assert!(create_cursor(&CursorImage::hidden()).unwrap().is_none());
    }

    #[test]
    fn a_cursor_outlives_its_image_while_a_window_shows_it() {
        let cache = CursorImageCache::default();
        let image = Arc::new(
            CursorImage::new(1, 1, (0, 0), CursorPixels::Bgra(vec![1, 2, 3, 255])).unwrap(),
        );
        let NativeCursor::Shown(cursor) = cache.cursor(&image) else {
            panic!("a visible cursor")
        };
        assert_eq!(
            cache.cursor(&image),
            NativeCursor::Shown(cursor),
            "made once"
        );
        drop(image);
        cache.prune(|shown| shown == cursor);
        assert_eq!(cache.entries.borrow().len(), 1);
        cache.prune(|_| false);
        assert!(cache.entries.borrow().is_empty());
    }
}
