//! Name-card overlay drawn into BGRX frames (M3).
//!
//! Bottom strip: a semi-transparent dark band plus white block text in a
//! built-in 5x7 bitmap font (uppercase ASCII, digits, basic punctuation).
//! No font crate — the glyph table below is the whole renderer.

const GW: usize = 5; // glyph width (columns)
const GH: usize = 7; // glyph height (rows)
const SCALE: usize = 3; // integer pixel scale -> 15x21 glyphs

/// Rows top->bottom; bit 4 = leftmost column.
fn glyph(c: u8) -> [u8; GH] {
    match c {
        b'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        b'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        b'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        b'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        b'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        b'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        b'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        b'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        b'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        b'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        b'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        b'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        b'N' => [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001],
        b'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        b'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        b'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        b'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        b'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        b'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        b'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
        b'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        b'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        b'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        b'0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        b'1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        b'3' => [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        b'4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        b'5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        b'6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        b'7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        b'8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        b'9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        b'.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00110],
        b',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00100, 0b01000],
        b':' => [0b00000, 0b00110, 0b00110, 0b00000, 0b00110, 0b00110, 0b00000],
        b'!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        b'?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100],
        b'-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        b'_' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111],
        b'+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
        b'@' => [0b01110, 0b10001, 0b10111, 0b10101, 0b10111, 0b10000, 0b01110],
        b'/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        b'(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        b')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        b'&' => [0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101],
        b'\'' => [0b00100, 0b00100, 0b01000, 0b00000, 0b00000, 0b00000, 0b00000],
        b'|' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        _ => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
    }
}

/// Draw `text` on a semi-transparent band across the bottom of `frame`
/// (tightly-packed BGRX, `w`x`h`). Text is centered horizontally and
/// vertically inside the band; unsupported chars render blank.
pub fn draw_name_card(frame: &mut [u8], w: usize, h: usize, text: &str) {
    let text_h = GH * SCALE;
    let band_h = (text_h + 16).min(h);
    let band_top = h - band_h;

    // Band: 75% dark slate over the frame. BGR order.
    const BAND: (u16, u16, u16) = (46, 38, 34);
    const A: u16 = 190;
    const IA: u16 = 255 - A;
    for y in band_top..h {
        let row = &mut frame[y * w * 4..(y + 1) * w * 4];
        for px in row.chunks_exact_mut(4) {
            px[0] = ((px[0] as u16 * IA + BAND.0 * A) / 255) as u8;
            px[1] = ((px[1] as u16 * IA + BAND.1 * A) / 255) as u8;
            px[2] = ((px[2] as u16 * IA + BAND.2 * A) / 255) as u8;
        }
    }
    // 2px accent line on the band's top edge (teal, near-opaque).
    const ACCENT: (u16, u16, u16) = (160, 120, 30);
    for y in band_top..(band_top + 2).min(h) {
        let row = &mut frame[y * w * 4..(y + 1) * w * 4];
        for px in row.chunks_exact_mut(4) {
            px[0] = ((px[0] as u16 * 35 + ACCENT.0 * 220) / 255) as u8;
            px[1] = ((px[1] as u16 * 35 + ACCENT.1 * 220) / 255) as u8;
            px[2] = ((px[2] as u16 * 35 + ACCENT.2 * 220) / 255) as u8;
        }
    }

    // Text: uppercase, centered; advance = (GW+1)*SCALE per char.
    let chars: Vec<u8> = text.bytes().map(|c| c.to_ascii_uppercase()).collect();
    let advance = (GW + 1) * SCALE;
    let text_w = chars.len() * advance;
    if text_w == 0 || text_w > w {
        return;
    }
    let x0 = (w - text_w) / 2;
    let y0 = band_top + (band_h - text_h) / 2;
    for (i, c) in chars.iter().enumerate() {
        let g = glyph(*c);
        for (ry, row) in g.iter().enumerate() {
            for bx in 0..GW {
                if row & (1 << (GW - 1 - bx)) == 0 {
                    continue;
                }
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let x = x0 + i * advance + bx * SCALE + dx;
                        let y = y0 + ry * SCALE + dy;
                        if x < w && y < h {
                            let d = (y * w + x) * 4;
                            frame[d] = 0xFF;
                            frame[d + 1] = 0xFF;
                            frame[d + 2] = 0xFF;
                        }
                    }
                }
            }
        }
    }
}
