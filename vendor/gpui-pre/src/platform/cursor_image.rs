//! Cursor shapes supplied by the application instead of a [`CursorStyle`](crate::CursorStyle).
//!
//! Local patch, not part of upstream GPUI: see `PATCHES.md` at the crate root.
//! A remote desktop client has to show whatever cursor the remote machine
//! shows — including the classic AND/XOR cursors that invert the screen under
//! them (the text I-beam) — and hand it to the platform as a real system
//! cursor so it tracks the mouse with no frame of latency.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

use anyhow::{Result, ensure};

/// Identity of a [`CursorImage`]. Platforms key their native cursor handles by it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CursorImageId(u64);

/// The pixels of a [`CursorImage`]. Rows run top-down, left to right.
pub enum CursorPixels {
    /// Nothing is drawn: the cursor disappears while this image is in effect.
    Hidden,
    /// Four bytes per pixel in B, G, R, A order, with straight (not
    /// premultiplied) alpha.
    Bgra(Vec<u8>),
    /// A classic AND/XOR cursor. Each pixel combines with the screen as
    /// `screen = (screen AND and) XOR xor`: an `and` pixel over black leaves the
    /// screen as it is, an `and` pixel over white inverts it, and a pixel
    /// without `and` is the `xor` color itself.
    Masked {
        /// One entry per pixel; `true` keeps the screen pixel under the cursor.
        and: Vec<bool>,
        /// Four bytes per pixel in B, G, R, X order; the fourth byte is ignored.
        xor: Vec<u8>,
    },
}

/// A cursor shape supplied by the application.
///
/// Set it on a hitbox with [`Window::set_cursor_image`](crate::Window::set_cursor_image).
/// Platforms that can't show application cursors use the fallback
/// [`CursorStyle`](crate::CursorStyle) given there; ask
/// [`App::supports_cursor_images`](crate::App::supports_cursor_images) first to
/// draw a substitute yourself.
pub struct CursorImage {
    id: CursorImageId,
    width: u32,
    height: u32,
    hotspot: (u32, u32),
    pixels: CursorPixels,
}

impl CursorImage {
    /// Checks that `pixels` holds exactly `width × height` pixels. The hotspot
    /// is clamped into the image.
    pub fn new(width: u32, height: u32, hotspot: (u32, u32), pixels: CursorPixels) -> Result<Self> {
        let count = width as usize * height as usize;
        match &pixels {
            CursorPixels::Hidden => {}
            CursorPixels::Bgra(bgra) => {
                ensure!(count > 0, "empty cursor image");
                ensure!(
                    bgra.len() == count * 4,
                    "cursor image of {width}×{height} needs {} BGRA bytes, got {}",
                    count * 4,
                    bgra.len()
                );
            }
            CursorPixels::Masked { and, xor } => {
                ensure!(count > 0, "empty cursor image");
                ensure!(
                    and.len() == count && xor.len() == count * 4,
                    "cursor image of {width}×{height} needs {count} mask entries and {} color \
                     bytes, got {} and {}",
                    count * 4,
                    and.len(),
                    xor.len()
                );
            }
        }
        Ok(Self::with_id(
            width,
            height,
            (
                hotspot.0.min(width.saturating_sub(1)),
                hotspot.1.min(height.saturating_sub(1)),
            ),
            pixels,
        ))
    }

    /// A cursor that hides the pointer.
    pub fn hidden() -> Self {
        Self::with_id(0, 0, (0, 0), CursorPixels::Hidden)
    }

    fn with_id(width: u32, height: u32, hotspot: (u32, u32), pixels: CursorPixels) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Self {
            id: CursorImageId(NEXT_ID.fetch_add(1, SeqCst)),
            width,
            height,
            hotspot,
            pixels,
        }
    }

    /// Unique for the life of the process.
    pub fn id(&self) -> CursorImageId {
        self.id
    }

    /// Width in device pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in device pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixel that sits under the pointer position, from the top-left corner.
    pub fn hotspot(&self) -> (u32, u32) {
        self.hotspot
    }

    /// The pixels, see [`CursorPixels`].
    pub fn pixels(&self) -> &CursorPixels {
        &self.pixels
    }

    /// Whether this image hides the pointer.
    pub fn is_hidden(&self) -> bool {
        matches!(self.pixels, CursorPixels::Hidden)
    }
}

impl fmt::Debug for CursorImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.pixels {
            CursorPixels::Hidden => "hidden",
            CursorPixels::Bgra(_) => "bgra",
            CursorPixels::Masked { .. } => "masked",
        };
        f.debug_struct("CursorImage")
            .field("id", &self.id)
            .field("size", &(self.width, self.height))
            .field("hotspot", &self.hotspot)
            .field("pixels", &kind)
            .finish()
    }
}
