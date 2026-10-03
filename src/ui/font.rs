//! Fixed Small and Normal glyphs with a shared vertical-column catalog.
//!
//! Row definitions describe 3x5 Small and 5x7 Normal bitmaps. The highest
//! used row bit is the leftmost pixel. They also supply the independent
//! scalar rendering fixtures. A static initializer transposes these rows
//! into the column order consumed by the page-oriented text renderer.
//!
//! The catalog covers ASCII 32..93 inclusive, including both brackets.
//! Each entry contains three Small columns followed by five Normal columns.
//! Large doubles the Normal columns without another font table. Bit zero
//! of a column is its top pixel; unused bottom bits remain clear.
//!
//! Lowercase maps to uppercase. Unsupported bytes use the question-mark
//! bitmap. The catalog includes fallback entries for unsupported characters
//! inside its contiguous range, avoiding a separate character-index table.
//!
//! There is one immutable 496-byte catalog and no runtime initialization,
//! glyph cache, UTF-8 decoder, proportional spacing, or writable font state.

pub(super) const fn rows(mut byte: u8) -> [u8; 7] {
    if byte >= b'a' && byte <= b'z' {
        byte -= 32;
    }
    match byte {
        b' ' => [0, 0, 0, 0, 0, 0, 0],
        b'0' => [14, 17, 19, 21, 25, 17, 14],
        b'1' => [4, 12, 4, 4, 4, 4, 14],
        b'2' => [14, 17, 1, 2, 4, 8, 31],
        b'3' => [30, 1, 1, 14, 1, 1, 30],
        b'4' => [2, 6, 10, 18, 31, 2, 2],
        b'5' => [31, 16, 16, 30, 1, 1, 30],
        b'6' => [14, 16, 16, 30, 17, 17, 14],
        b'7' => [31, 1, 2, 4, 8, 8, 8],
        b'8' => [14, 17, 17, 14, 17, 17, 14],
        b'9' => [14, 17, 17, 15, 1, 1, 14],
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'B' => [30, 17, 17, 30, 17, 17, 30],
        b'C' => [14, 17, 16, 16, 16, 17, 14],
        b'D' => [30, 17, 17, 17, 17, 17, 30],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'F' => [31, 16, 16, 30, 16, 16, 16],
        b'G' => [14, 17, 16, 23, 17, 17, 15],
        b'H' => [17, 17, 17, 31, 17, 17, 17],
        b'I' => [14, 4, 4, 4, 4, 4, 14],
        b'J' => [7, 2, 2, 2, 2, 18, 12],
        b'K' => [17, 18, 20, 24, 20, 18, 17],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'M' => [17, 27, 21, 21, 17, 17, 17],
        b'N' => [17, 25, 21, 19, 17, 17, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'P' => [30, 17, 17, 30, 16, 16, 16],
        b'Q' => [14, 17, 17, 17, 21, 18, 13],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'S' => [15, 16, 16, 14, 1, 1, 30],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'V' => [17, 17, 17, 17, 17, 10, 4],
        b'W' => [17, 17, 17, 21, 21, 21, 10],
        b'X' => [17, 17, 10, 4, 10, 17, 17],
        b'Y' => [17, 17, 10, 4, 4, 4, 4],
        b'Z' => [31, 1, 2, 4, 8, 16, 31],
        b'-' => [0, 0, 0, 31, 0, 0, 0],
        b'+' => [0, 4, 4, 31, 4, 4, 0],
        b'=' => [0, 0, 31, 0, 31, 0, 0],
        b':' => [0, 4, 4, 0, 4, 4, 0],
        b'.' => [0, 0, 0, 0, 0, 4, 4],
        b'/' => [1, 2, 2, 4, 8, 8, 16],
        b'<' => [1, 2, 4, 8, 4, 2, 1],
        b'>' => [16, 8, 4, 2, 4, 8, 16],
        b'*' => [0, 21, 14, 31, 14, 21, 0],
        b'[' => [14, 8, 8, 8, 8, 8, 14],
        b']' => [14, 2, 2, 2, 2, 2, 14],
        _ => [14, 17, 1, 2, 4, 0, 4],
    }
}

/// Small has independent 3x5 glyphs; Normal uses 5x7 glyphs.
/// Large doubles the Normal bitmap without introducing another font table.
/// Cell dimensions include the cleared inter-character column and bottom row.
#[derive(Clone, Copy)]
pub(crate) enum Size {
    Small,
    Normal,
    Large,
}

impl Size {
    pub(crate) const fn cell(self) -> (usize, usize) {
        match self {
            Self::Small => (4, 6),
            Self::Normal => (6, 8),
            Self::Large => (12, 16),
        }
    }

    pub(super) const fn source(self) -> (usize, usize, usize) {
        match self {
            Self::Small => (3, 5, 1),
            Self::Normal => (5, 7, 1),
            Self::Large => (5, 7, 2),
        }
    }
}

const fn compact(mut byte: u8) -> [u8; 5] {
    if byte >= b'a' && byte <= b'z' {
        byte -= 32;
    }
    match byte {
        b' ' => [0, 0, 0, 0, 0],
        b'0' => [7, 5, 5, 5, 7],
        b'1' => [2, 6, 2, 2, 7],
        b'2' => [7, 1, 7, 4, 7],
        b'3' => [7, 1, 7, 1, 7],
        b'4' => [5, 5, 7, 1, 1],
        b'5' => [7, 4, 7, 1, 7],
        b'6' => [7, 4, 7, 5, 7],
        b'7' => [7, 1, 2, 2, 2],
        b'8' => [7, 5, 7, 5, 7],
        b'9' => [7, 5, 7, 1, 7],
        b'A' => [2, 5, 7, 5, 5],
        b'B' => [6, 5, 6, 5, 6],
        b'C' => [3, 4, 4, 4, 3],
        b'D' => [6, 5, 5, 5, 6],
        b'E' => [7, 4, 6, 4, 7],
        b'F' => [7, 4, 6, 4, 4],
        b'G' => [3, 4, 5, 5, 3],
        b'H' => [5, 5, 7, 5, 5],
        b'I' => [7, 2, 2, 2, 7],
        b'J' => [1, 1, 1, 5, 2],
        b'K' => [5, 5, 6, 5, 5],
        b'L' => [4, 4, 4, 4, 7],
        b'M' => [5, 7, 7, 5, 5],
        b'N' => [5, 7, 7, 7, 5],
        b'O' => [2, 5, 5, 5, 2],
        b'P' => [6, 5, 6, 4, 4],
        b'Q' => [2, 5, 5, 3, 1],
        b'R' => [6, 5, 6, 5, 5],
        b'S' => [3, 4, 2, 1, 6],
        b'T' => [7, 2, 2, 2, 2],
        b'U' => [5, 5, 5, 5, 7],
        b'V' => [5, 5, 5, 5, 2],
        b'W' => [5, 5, 7, 7, 5],
        b'X' => [5, 5, 2, 5, 5],
        b'Y' => [5, 5, 2, 2, 2],
        b'Z' => [7, 1, 2, 4, 7],
        b'-' => [0, 0, 7, 0, 0],
        b'+' => [0, 2, 7, 2, 0],
        b'=' => [0, 7, 0, 7, 0],
        b':' => [0, 2, 0, 2, 0],
        b'.' => [0, 0, 0, 0, 2],
        b'/' => [1, 1, 2, 4, 4],
        b'<' => [1, 2, 4, 2, 1],
        b'>' => [4, 2, 1, 2, 4],
        b'*' => [0, 5, 2, 5, 0],
        b'[' => [3, 2, 2, 2, 3],
        b']' => [6, 2, 2, 2, 6],
        _ => [7, 1, 2, 0, 2],
    }
}

/// Transpose both source bitmaps into one eight-byte catalog entry.
/// This function is used by the static initializer, not by frame drawing.
const fn vertical_catalog() -> [[u8; 8]; 62] {
    let mut table = [[0u8; 8]; 62];
    let mut index = 0;

    while index < table.len() {
        let byte = b' ' + index as u8;
        let mut family = 0;

        while family < 2 {
            let size = if family == 0 { Size::Small } else { Size::Normal };
            let source = glyph(byte, size);
            let (columns, rows, _) = size.source();
            let offset = if family == 0 { 0 } else { 3 };

            let mut column = 0;
            while column < columns {
                let selected = 1u8 << (columns - 1 - column);
                let mut bits = 0u8;
                let mut row = 0;

                while row < rows {
                    if source[row] & selected != 0 {
                        bits |= 1u8 << row;
                    }
                    row += 1;
                }

                table[index][offset + column] = bits;
                column += 1;
            }

            family += 1;
        }

        index += 1;
    }

    table
}

static VERTICAL: [[u8; 8]; 62] = vertical_catalog();

/// Borrow three or five columns directly from immutable storage. The slice
/// contains glyph pixels only; the renderer supplies the blank spacing
/// column and opaque bottom margin. Large borrows the Normal bitmap.
///
/// Character normalization is done once per glyph, not once per column.
/// No reference to mutable storage or peripheral memory is involved.
pub(super) const fn vertical(mut byte: u8, size: Size) -> &'static [u8] {
    if byte >= b'a' && byte <= b'z' {
        byte -= 32;
    }
    if byte < b' ' || byte > b']' {
        byte = b'?';
    }

    let columns = &VERTICAL[(byte - b' ') as usize];
    match size {
        Size::Small => columns.split_at(3).0,
        Size::Normal | Size::Large => columns.split_at(3).1,
    }
}

/// Spread eight input bits into the even positions of a sixteen-bit word,
/// then duplicate them into the adjacent odd positions. Every source pixel
/// becomes two vertical pixels. Normal glyphs use only seven source bits,
/// leaving the doubled bottom margin clear.
pub(super) const fn double_column(value: u8) -> u16 {
    let mut bits = value as u16;
    bits = (bits | (bits << 4)) & 0x0F0F;
    bits = (bits | (bits << 2)) & 0x3333;
    bits = (bits | (bits << 1)) & 0x5555;
    bits | (bits << 1)
}

/// Compare catalog output with direct expansion of the row definitions.
/// Both supported and fallback bytes participate. The expected columns
/// are computed without the catalog builder or double_column.
const fn check_vertical(first: u16, end: u16) {
    assert!(first <= end && end <= 256);
    let sizes = [Size::Small, Size::Normal, Size::Large];
    let mut byte = first;

    while byte < end {
        let mut size_index = 0;
        while size_index < sizes.len() {
            let size = sizes[size_index];
            let source = glyph(byte as u8, size);
            let columns = vertical(byte as u8, size);
            let (width, height) = size.cell();
            let (source_columns, rows, scale) = size.source();

            assert!(columns.len() == source_columns);
            assert!(width == (columns.len() + 1) * scale);
            assert!(height == (rows + 1) * scale);

            let mut column = 0;
            while column < columns.len() {
                let actual = if scale == 2 {
                    double_column(columns[column])
                } else {
                    columns[column] as u16
                };
                let mut expected = 0u16;
                let mut row = 0;

                while row < height {
                    let source_row = row / scale;
                    if source_row < rows
                        && source[source_row] & (1 << (source_columns - 1 - column)) != 0
                    {
                        expected |= 1u16 << row;
                    }
                    row += 1;
                }

                assert!(actual == expected);
                column += 1;
            }

            size_index += 1;
        }

        byte += 1;
    }
}

const _: () = check_vertical(0, 64);
const _: () = check_vertical(64, 128);
const _: () = check_vertical(128, 192);
const _: () = check_vertical(192, 256);

const _: () = {
    assert!(core::mem::size_of::<[[u8; 8]; 62]>() == 496);
    assert!(VERTICAL.len() == (b']' - b' ' + 1) as usize);

    // Exercise every possible byte independently of the font bitmaps.
    let mut value = 0u16;
    while value < 256 {
        let mut expected = 0u16;
        let mut bit = 0;
        while bit < 8 {
            if value & (1u16 << bit) != 0 {
                expected |= 3u16 << (bit * 2);
            }
            bit += 1;
        }
        assert!(double_column(value as u8) == expected);
        value += 1;
    }

    assert!(double_column(0) == 0);
    assert!(double_column(1) == 3);
    assert!(double_column(0x7F) == 0x3FFF);
    assert!(double_column(0x80) == 0xC000);
    assert!(double_column(0xFF) == 0xFFFF);

    let small_space = vertical(b' ', Size::Small);
    let normal_space = vertical(b' ', Size::Normal);
    let mut column = 0;
    while column < normal_space.len() {
        assert!(normal_space[column] == 0);
        if column < small_space.len() {
            assert!(small_space[column] == 0);
        }
        column += 1;
    }
};

pub(super) const fn glyph(byte: u8, size: Size) -> [u8; 7] {
    match size {
        Size::Small => {
            let small = compact(byte);
            [small[0], small[1], small[2], small[3], small[4], 0, 0]
        }
        Size::Normal | Size::Large => rows(byte),
    }
}

const _: () = {
    assert!(Size::Small.cell().0 == 4 && Size::Small.cell().1 == 6);
    assert!(Size::Normal.cell().0 == 6 && Size::Normal.cell().1 == 8);
    assert!(Size::Large.cell().0 == 12 && Size::Large.cell().1 == 16);

    let mut byte = 0u16;
    while byte < 256 {
        let small = compact(byte as u8);
        let mut row = 0;
        while row < 5 {
            assert!(small[row] < 8);
            row += 1;
        }
        byte += 1;
    }

    let upper = rows(b'A');
    let lower = rows(b'a');
    let mut index = 0;
    while index < 7 {
        assert!(upper[index] == lower[index]);
        index += 1;
    }

    let fallback = rows(255);
    let question = rows(b'?');
    index = 0;
    while index < 7 {
        assert!(fallback[index] == question[index]);
        index += 1;
    }

    let mut byte = 0u16;
    while byte < 256 {
        let glyph = rows(byte as u8);
        index = 0;
        while index < 7 {
            assert!(glyph[index] < 32);
            index += 1;
        }
        byte += 1;
    }
};