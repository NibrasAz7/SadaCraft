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

// --- M3: subtitle + teleprompter overlays -----------------------------------

/// Which edge a text band hugs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BandPos {
    Top,
    Bottom,
}

const TP_SCALE: usize = 2; // 10x14 glyphs — more columns/lines than name card
const TP_VISIBLE_LINES: usize = 4;
const TP_LINE_PAD: usize = 4; // extra px between teleprompter lines
const TP_PAD_X: usize = 8; // left margin inside the band
const TP_PAD_Y: usize = 6; // top margin inside the band

/// Alpha-blend `bgr` over a horizontal strip of the frame.
fn blend_band(frame: &mut [u8], w: usize, top: usize, height: usize, bgr: (u16, u16, u16), a: u16) {
    let ia = 255 - a;
    let bottom = (top + height).min(frame.len() / (w * 4).max(1));
    for y in top..bottom {
        let row = &mut frame[y * w * 4..(y + 1) * w * 4];
        for px in row.chunks_exact_mut(4) {
            px[0] = ((px[0] as u16 * ia + bgr.0 * a) / 255) as u8;
            px[1] = ((px[1] as u16 * ia + bgr.1 * a) / 255) as u8;
            px[2] = ((px[2] as u16 * ia + bgr.2 * a) / 255) as u8;
        }
    }
}

/// Draw uppercase `chars` at `scale`, pixels clipped to `clip_top <= y < clip_bot`
/// (vertical clip lets teleprompter lines scroll inside the band). `y0` may be
/// negative while a line scrolls out of view.
fn draw_chars(
    frame: &mut [u8],
    w: usize,
    h: usize,
    x0: usize,
    y0: i32,
    chars: &[u8],
    scale: usize,
    clip_top: usize,
    clip_bot: usize,
) {
    let advance = (GW + 1) * scale;
    for (i, c) in chars.iter().enumerate() {
        let g = glyph(*c);
        for (ry, row) in g.iter().enumerate() {
            for bx in 0..GW {
                if row & (1 << (GW - 1 - bx)) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    let y = y0 + (ry * scale + dy) as i32;
                    if y < clip_top as i32 || y >= clip_bot as i32 {
                        continue;
                    }
                    for dx in 0..scale {
                        let x = x0 + i * advance + bx * scale + dx;
                        if x < w && (y as usize) < h {
                            let d = (y as usize * w + x) * 4;
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

/// Single centered subtitle line on a translucent bottom band
/// (`--subtitle` / `--subtitle-file`). Text longer than the frame is
/// truncated to fit.
pub fn draw_subtitle(frame: &mut [u8], w: usize, h: usize, text: &str) {
    let text_h = GH * SCALE;
    let band_h = (text_h + 14).min(h);
    let band_top = h - band_h;
    blend_band(frame, w, band_top, band_h, (30, 28, 26), 170);

    let advance = (GW + 1) * SCALE;
    let max_chars = w / advance;
    let mut chars: Vec<u8> = text.bytes().map(|c| c.to_ascii_uppercase()).collect();
    chars.truncate(max_chars);
    if chars.is_empty() {
        return;
    }
    let x0 = (w - chars.len() * advance) / 2;
    let y0 = (band_top + (band_h - text_h) / 2) as i32;
    draw_chars(frame, w, h, x0, y0, &chars, SCALE, 0, h);
}

/// Word-wrapped scrolling text (`--teleprompter`). Holds the wrapped lines;
/// `draw` renders the window of lines at `scroll_px` inside a translucent
/// top or bottom band. Scroll is clamped so the last line stops visible.
pub struct Teleprompter {
    lines: Vec<String>,
}

impl Teleprompter {
    /// Wrap `text` to the columns that fit `frame_w` at the teleprompter
    /// glyph scale (word wrap; overlong words are hard-split).
    pub fn from_text(text: &str, frame_w: usize) -> Self {
        let advance = (GW + 1) * TP_SCALE;
        let cpl = ((frame_w - 2 * TP_PAD_X) / advance).max(8);
        let mut lines = Vec::new();
        for raw in text.lines() {
            let mut cur = String::new();
            for word in raw.split_whitespace() {
                if cur.is_empty() {
                    cur = word.to_string();
                } else if cur.len() + 1 + word.len() <= cpl {
                    cur.push(' ');
                    cur.push_str(word);
                } else {
                    lines.push(std::mem::take(&mut cur));
                    cur = word.to_string();
                }
                // Hard-split words that alone exceed a line.
                while cur.len() > cpl {
                    lines.push(cur[..cpl].to_string());
                    cur = cur[cpl..].to_string();
                }
            }
            lines.push(cur); // keeps blank paragraph lines as spacing
        }
        Teleprompter { lines }
    }

    /// Pixels per teleprompter line (glyph + padding).
    pub fn line_height() -> usize {
        GH * TP_SCALE + TP_LINE_PAD
    }

    /// Render the band and the visible line window. `scroll_px` is the
    /// scroll offset in pixels (e.g. seconds * speed).
    pub fn draw(&self, frame: &mut [u8], w: usize, h: usize, scroll_px: usize, pos: BandPos) {
        if self.lines.is_empty() {
            return;
        }
        let lh = Self::line_height();
        let band_h = (TP_VISIBLE_LINES * lh + 2 * TP_PAD_Y).min(h);
        let band_top = match pos {
            BandPos::Top => 0,
            BandPos::Bottom => h - band_h,
        };
        blend_band(frame, w, band_top, band_h, (34, 30, 28), 200);

        // Clamp so the tail of the script stays on screen instead of
        // scrolling into emptiness.
        let max_scroll = self
            .lines
            .len()
            .saturating_sub(TP_VISIBLE_LINES)
            .saturating_mul(lh);
        let scroll_px = scroll_px.min(max_scroll);
        let first = scroll_px / lh;
        let off = (scroll_px % lh) as i32;
        for i in 0..TP_VISIBLE_LINES {
            let Some(line) = self.lines.get(first + i) else {
                break;
            };
            let chars: Vec<u8> = line.bytes().map(|c| c.to_ascii_uppercase()).collect();
            let y0 = band_top as i32 + TP_PAD_Y as i32 + (i * lh) as i32 - off;
            draw_chars(frame, w, h, TP_PAD_X, y0, &chars, TP_SCALE, band_top, band_top + band_h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teleprompter_wraps_words() {
        let tp = Teleprompter::from_text("hello world this is a test", 120);
        // 120px -> cpl = (120-16)/12 = 8 chars/line, hard wrap on words.
        assert!(tp.lines.iter().all(|l| l.len() <= 8 || !l.contains(' ')));
        assert!(tp.lines.len() > 1);
    }

    #[test]
    fn teleprompter_draw_smoke() {
        let (w, h) = (160usize, 60usize);
        let tp = Teleprompter::from_text("one two three four five six seven eight nine ten", w);
        let mut frame = vec![0u8; w * h * 4];
        tp.draw(&mut frame, w, h, 0, BandPos::Top);
        tp.draw(&mut frame, w, h, usize::MAX, BandPos::Bottom); // clamped, no panic
    }

    #[test]
    fn subtitle_smoke() {
        let (w, h) = (160usize, 60usize);
        let mut frame = vec![0u8; w * h * 4];
        draw_subtitle(&mut frame, w, h, "hello");
        assert!(frame.iter().any(|&b| b != 0)); // band + text landed
    }
}
