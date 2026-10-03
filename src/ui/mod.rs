//! One 128 by 64 page-ordered framebuffer.
//!
//! A byte holds eight vertical pixels; bit zero is the top pixel of its
//! page. The buffer therefore matches LCD page writes without conversion.
//! Drawing clips to the visible rectangle. Rectangle operations update
//! masked page bytes; text writes opaque vertical columns spanning at most
//! three pages. Neither path allocates another framebuffer.
//!
//! Text processes at most one screen width of cells plus one extra cell:
//! 33 Small, 22 Normal, or 11 Large cells. This bound applies even when a
//! negative origin places some of those cells outside the visible area.
//!
//! The foreground owns the buffer. It must not draw while a frame transfer
//! is active: there is no second buffer or per-page snapshot.

pub(crate) mod font;
pub(crate) mod widgets;
mod raster_checks;

use core::ptr::addr_of_mut;

#[repr(C)]
pub(crate) struct Frame {
    bytes: [u8; 1024],
}

#[derive(Clone, Copy)]
enum RectOp {
    Clear,
    Set,
    Toggle,
}

impl Frame {
    const fn new() -> Self {
        Self { bytes: [0; 1024] }
    }

    pub(crate) const fn clear(&mut self) {
        let mut index = 0;
        while index < self.bytes.len() {
            self.bytes[index] = 0;
            index += 1;
        }
    }

    pub(crate) const fn pixel(&mut self, x: i32, y: i32, on: bool) {
        if x < 0 || x >= 128 || y < 0 || y >= 64 {
            return;
        }
        let index = (y as usize / 8) * 128 + x as usize;
        let bit = 1u8 << (y as u32 & 7);
        if on {
            self.bytes[index] |= bit;
        } else {
            self.bytes[index] &= !bit;
        }
    }

    /// Shared rectangle engine for fill, outline edges, and inversion.
    ///
    /// Private callers derive coordinates from i16 origins and u16 extents,
    /// including translated outline edges. Their sums remain inside i32.
    /// Clipping and the empty-intersection check precede unsigned indexing.
    ///
    /// First and last page masks preserve pixels outside the rectangle.
    /// A one-page rectangle combines both masks. The inner loop uses
    /// (old AND keep) XOR change for all three operations, with no per-pixel
    /// branch and no read or write outside the selected page-byte interval.
    #[inline(never)]
    const fn rectangle(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        operation: RectOp,
    ) {
        if width <= 0 || height <= 0 {
            return;
        }

        let left = if x < 0 { 0 } else { x };
        let top = if y < 0 { 0 } else { y };
        let right = if x + width > 128 { 128 } else { x + width };
        let bottom = if y + height > 64 { 64 } else { y + height };

        if left >= right || top >= bottom {
            return;
        }

        let first = top as usize >> 3;
        let last = (bottom as usize - 1) >> 3;
        let first_mask = 0xFFu8 << (top as u32 & 7);
        let last_mask = 0xFFu8 >> ((8 - (bottom as u32 & 7)) & 7);

        let mut page = first;
        while page <= last {
            let mut mask = if page == first { first_mask } else { 0xFF };
            if page == last {
                mask &= last_mask;
            }

            let (keep, change) = match operation {
                RectOp::Clear => (!mask, 0),
                RectOp::Set => (!mask, mask),
                RectOp::Toggle => (0xFF, mask),
            };

            let mut at = page * 128 + left as usize;
            let end = page * 128 + right as usize;
            while at < end {
                self.bytes[at] = (self.bytes[at] & keep) ^ change;
                at += 1;
            }

            page += 1;
        }
    }

    const fn clipped_fill(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        on: bool,
    ) {
        self.rectangle(
            x, y, width, height,
            if on { RectOp::Set } else { RectOp::Clear },
        );
    }

    pub(crate) const fn fill(
        &mut self,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        on: bool,
    ) {
        self.clipped_fill(x as i32, y as i32, width as i32, height as i32, on);
    }

    /// Four clipped edges with no retained drawing state. The runtime
    /// boundary favors sharing edge setup across screen renderers.
    #[inline(never)]
    pub(crate) const fn outline(
        &mut self,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        on: bool,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let x = x as i32;
        let y = y as i32;
        let width = width as i32;
        let height = height as i32;
        self.clipped_fill(x, y, width, 1, on);
        self.clipped_fill(x, y + height - 1, width, 1, on);
        self.clipped_fill(x, y, 1, height, on);
        self.clipped_fill(x + width - 1, y, 1, height, on);
    }

    /// Draws opaque cells. Both the inter-character column and the eighth
    /// row are cleared, so replacing a glyph does not retain old pixels.
    pub(crate) const fn text(&mut self, x: i16, y: i16, text: &[u8]) {
        self.text_size(x, y, text, font::Size::Normal);
    }

    /// Replace one opaque text column. Bit zero is its top source pixel.
    ///
    /// Text cells have heights 6, 8, or 16. Rejecting other heights bounds
    /// the temporary mask to 23 bits after page alignment. Negative y clips
    /// source bits before alignment; a fully clipped column performs no I/O.
    /// Unselected bits in the first and last page bytes are preserved.
    ///
    /// Only scalar values are temporary. The column occupies at most three
    /// destination bytes and does not require a glyph or framebuffer cache.
    #[inline(never)]
    const fn text_column(&mut self, x: i32, y: i16, height: usize, bits: u16) {
        if x < 0 || x >= 128 || height == 0 || height > 16 {
            return;
        }

        let y = y as i32;
        let top = if y < 0 { 0 } else { y };
        let bottom = if y + height as i32 > 64 {
            64
        } else {
            y + height as i32
        };
        if top >= bottom {
            return;
        }

        let shift = top as u32 & 7;
        let count = (bottom - top) as u32;
        let skipped = (top - y) as u32;
        let mut mask = ((1u32 << count) - 1) << shift;
        let mut data = ((bits as u32) >> skipped) << shift;
        let mut at = (top as usize >> 3) * 128 + x as usize;

        while mask != 0 {
            let selected = mask as u8;
            self.bytes[at] = (self.bytes[at] & !selected) | (data as u8 & selected);
            mask >>= 8;
            data >>= 8;
            at += 128;
        }
    }

    /// Opaque, clipped cells with the same bounded input traversal for all
    /// origins. Glyph columns are borrowed from the immutable font catalog;
    /// frame composition performs no row-to-column conversion.
    ///
    /// Large expands a column's bits vertically and writes it at two adjacent
    /// horizontal positions. The final column is blank spacing. Unused bottom
    /// bits are zero, so the entire cell remains opaque, including its margin.
    /// All clipping and page-mask handling remain in text_column.
    pub(crate) const fn text_size(
        &mut self,
        x: i16,
        y: i16,
        text: &[u8],
        size: font::Size,
    ) {
        let (width, height) = size.cell();
        let scale = if matches!(size, font::Size::Large) { 2 } else { 1 };
        let limit = 128 / width + 1;
        let count = if text.len() < limit { text.len() } else { limit };

        let mut index = 0;
        while index < count {
            let left = x as i32 + (index * width) as i32;
            if left >= 128 {
                break;
            }
            let glyph = font::vertical(text[index], size);

            let mut column = 0;
            while column <= glyph.len() {
                let raw = if column < glyph.len() { glyph[column] } else { 0 };
                let bits = if scale == 2 {
                    font::double_column(raw)
                } else {
                    raw as u16
                };

                let at = left + (column * scale) as i32;
                self.text_column(at, y, height, bits);
                if scale == 2 {
                    self.text_column(at + 1, y, height, bits);
                }
                column += 1;
            }
            index += 1;
        }
    }

    /// Complements only the clipped rectangle through the shared page engine.
    /// Draw text first to obtain an opaque selected row without redrawing it.
    pub(crate) const fn invert(&mut self, x: i16, y: i16, width: u16, height: u16) {
        self.rectangle(
            x as i32, y as i32, width as i32, height as i32, RectOp::Toggle,
        );
    }

    /// Prints a decimal u32 followed by spaces in a ten-cell field.
    /// Runtime callers share the conversion and padding implementation;
    /// compiler fixtures evaluate the same body without a runtime call.
    #[inline(never)]
    pub(crate) const fn decimal(&mut self, x: i16, y: i16, mut value: u32) {
        let mut reversed = [0u8; 10];
        let mut length = 0;
        loop {
            reversed[length] = b'0' + (value % 10) as u8;
            length += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }

        let mut text = [b' '; 10];
        let mut index = 0;
        while index < length {
            text[index] = reversed[length - 1 - index];
            index += 1;
        }
        self.text(x, y, &text);
    }



    /// Eight uppercase hexadecimal cells; leading zeroes are retained.
    #[inline(never)]
    pub(crate) const fn hex(&mut self, x: i16, y: i16, value: u32) {
        let digits = b"0123456789ABCDEF";
        let mut text = [0u8; 8];
        let mut index = 0;
        while index < text.len() {
            text[index] = digits[((value >> ((7 - index) * 4)) & 15) as usize];
            index += 1;
        }
        self.text(x, y, &text);
    }

    pub(crate) const fn signed(&mut self, x: i16, y: i16, value: i32) {
        if value < 0 {
            self.text(x, y, b"-");
            self.decimal(x.saturating_add(6), y, value.unsigned_abs());
        } else {
            self.decimal(x, y, value as u32);
        }
    }

    pub(crate) fn byte(&self, index: usize) -> u8 {
        self.bytes[index]
    }
}

#[used]
#[no_mangle]
static mut TRAKT_FRAME: Frame = Frame::new();

/// Borrows the single static framebuffer.
///
/// call once after startup clears BSS. No interrupt or other
/// execution context may access TRAKT_FRAME while this reference exists.
pub(crate) unsafe fn frame() -> &'static mut Frame {
    unsafe { &mut *addr_of_mut!(TRAKT_FRAME) }
}

pub(crate) struct Snapshot {
    pub(crate) tick: u32,
    pub(crate) hclk: u32,
    pub(crate) pclk: u32,
    pub(crate) missed: u32,
    pub(crate) frames: u32,
}

/// Replaces the complete screen. The bottom bar follows a one-second
/// phase; it is a drawing diagnostic, not a battery or RF measurement.
pub(crate) const fn diagnostic(frame: &mut Frame, state: Snapshot) {
    frame.clear();
    frame.text(2, 0, b"TRAKT K5 V3");
    frame.fill(0, 8, 128, 1, true);

    frame.text(2, 10, b"HCLK");
    frame.decimal(42, 10, state.hclk);
    frame.text(2, 18, b"PCLK");
    frame.decimal(42, 18, state.pclk);
    frame.text(2, 26, b"TICK");
    frame.decimal(42, 26, state.tick);
    frame.text(2, 34, b"LOST");
    frame.decimal(42, 34, state.missed);
    frame.text(2, 42, b"LCD");
    frame.decimal(42, 42, state.frames);

    frame.text(2, 50, b"RX ONLY / TX BLOCKED");
    frame.outline(1, 60, 126, 4, true);
    let width = ((state.tick % 1_000) * 124 / 1_000) as u16;
    frame.fill(2, 61, width, 2, true);
}

const _: () = {
    assert!(core::mem::size_of::<Frame>() == 1024);

    let mut small = Frame::new();
    small.text_size(0, 0, b"0", font::Size::Small);
    assert!(small.bytes[0] == 31);
    assert!(small.bytes[1] == 17);
    assert!(small.bytes[2] == 31);
    assert!(small.bytes[3] == 0);

    let mut large = Frame::new();
    large.text_size(0, 0, b"A", font::Size::Large);
    assert!(large.bytes[0] == large.bytes[1]);
    assert!(large.bytes[2] == large.bytes[3]);
    assert!(large.bytes[128] == large.bytes[129]);
    large.invert(0, 0, 12, 16);
    large.invert(0, 0, 12, 16);
    assert!(large.bytes[0] == 0xFC);
    large.text_size(0, 0, b" ", font::Size::Large);
    assert!(large.bytes[0] == 0 && large.bytes[128] == 0);

    let mut frame = Frame::new();
    frame.pixel(127, 63, true);
    assert!(frame.bytes[1023] == 0x80);
    frame.pixel(-1, 0, true);
    frame.pixel(128, 0, true);
    frame.pixel(0, 64, true);
    assert!(frame.bytes[0] == 0);

    frame.fill(-2, -2, 4, 4, true);
    assert!(frame.bytes[0] == 3);
    assert!(frame.bytes[1] == 3);
    assert!(frame.bytes[2] == 0);

    frame.clear();
    frame.outline(10, 10, 3, 3, true);
    assert!(frame.bytes[128 + 10] == 0x1C);
    assert!(frame.bytes[128 + 11] == 0x14);
    assert!(frame.bytes[128 + 12] == 0x1C);

    frame.text(0, 0, b"A");
    assert!(frame.bytes[0] == 0x7E);
    assert!(frame.bytes[1] == 0x09);
    assert!(frame.bytes[5] == 0);
    frame.text(0, 0, b" ");
    assert!(frame.bytes[0] == 0);
    assert!(frame.bytes[1] == 0);

    frame.clear();
    frame.decimal(0, 0, 0);
    let mut zero = Frame::new();
    zero.text(0, 0, b"0");
    let mut index = 0;
    while index < 1024 {
        assert!(frame.bytes[index] == zero.bytes[index]);
        index += 1;
    }

    diagnostic(&mut frame, Snapshot {
        tick: u32::MAX,
        hclk: 72_000_000,
        pclk: 36_000_000,
        missed: u32::MAX,
        frames: u32::MAX,
    });
    assert!(frame.bytes[0] & 0x80 == 0);
    assert!(frame.bytes[128] & 1 == 1);
};