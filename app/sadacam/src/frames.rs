/// M1 synthetic frame generator: RGB32 (BGRX memory order) test pattern.
/// Color bars + a moving bright bar + frame-index stripes so motion and
/// frame progression are visible in any client preview.
pub fn fill_rgb32(buf: &mut [u8], width: usize, height: usize, stride: usize, frame_idx: u32) {
    const BARS: [(u8, u8, u8); 7] = [
        (0xC0, 0xC0, 0xC0), // gray
        (0xC0, 0xC0, 0x00), // yellow-ish (BGR)
        (0x00, 0xC0, 0xC0), // cyan
        (0x00, 0xC0, 0x00), // green
        (0xC0, 0x00, 0xC0), // magenta
        (0xC0, 0x00, 0x00), // red-ish
        (0x00, 0x00, 0xC0), // blue
    ];

    let bar_w = (width / BARS.len()).max(1);
    let moving_y = ((frame_idx as usize) * 4) % height;
    // Encode frame index in the first rows: 32 binary columns, readable as pattern.
    let counter_row = 8usize;

    for y in 0..height {
        let row = &mut buf[y * stride..y * stride + width * 4];
        for x in 0..width {
            let px = &mut row[x * 4..x * 4 + 4];
            if y == moving_y || y == moving_y + 1 {
                px[0] = 0xFF;
                px[1] = 0xFF;
                px[2] = 0xFF;
            } else if y < counter_row + 8 && y >= counter_row && x < 64 {
                // Frame counter visualized as 2 rows of bit-columns.
                let bit = (frame_idx >> ((x / 2) % 32)) & 1;
                let v = if bit == 1 { 0xFF } else { 0x20 };
                px[0] = v;
                px[1] = v;
                px[2] = v;
            } else {
                let (b, g, r) = BARS[(x / bar_w).min(BARS.len() - 1)];
                px[0] = b;
                px[1] = g;
                px[2] = r;
            }
            px[3] = 0xFF;
        }
    }
}

/// Flip a tightly-packed BGRX frame vertically in place. Media
/// Foundation consumers read RGB32 buffers bottom-up — producers publish
/// top-down images through this flip.
pub fn flip_vertical(frame: &mut [u8], w: usize, h: usize) {
    let row = w * 4;
    if frame.len() < row * h {
        return;
    }
    for y in 0..h / 2 {
        let (a, b) = (y * row, (h - 1 - y) * row);
        for x in 0..row {
            frame.swap(a + x, b + x);
        }
    }
}

/// Nearest-neighbor rescale of a tightly-packed BGRX frame into `dst`
/// (dw x dh). Used when the webcam negotiates a size different from the
/// shm slot.
pub fn resize_bgrx_nearest(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize) {
    if src.len() < sw * sh * 4 || dst.len() < dw * dh * 4 {
        return;
    }
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            let (s, d) = ((sy * sw + sx) * 4, (y * dw + x) * 4);
            dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
}

#[inline]
fn clamp8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// BGRX -> NV12, BT.601 limited range — the inverse of
/// `webcam::nv12_to_bgrx`. `src` is tightly packed (stride w*4);
/// `src_bottom_up` mirrors the RGB32 DIB convention used by the shm
/// feed (producers publish pre-flipped so MF clients see the image
/// upright). NV12 output is always top-down: `w*h` Y bytes then `w*h/2`
/// interleaved UV bytes. `w`/`h` must be even for the chroma pass;
/// odd tails are left unwritten in `dst`.
pub fn bgrx_to_nv12(src: &[u8], w: usize, h: usize, dst: &mut [u8], src_bottom_up: bool) {
    if src.len() < w * h * 4 || dst.len() < w * h * 3 / 2 {
        return;
    }
    let (y_plane, uv_plane) = dst.split_at_mut(w * h);
    let src_row = |y: usize| if src_bottom_up { h - 1 - y } else { y };
    for y in 0..h {
        let srow = src_row(y) * w * 4;
        for x in 0..w {
            let s = srow + x * 4;
            let (b, g, r) = (src[s] as i32, src[s + 1] as i32, src[s + 2] as i32);
            y_plane[y * w + x] = clamp8(((66 * r + 129 * g + 25 * b + 128) >> 8) + 16);
        }
    }
    // 4:2:0 chroma: average the 2x2 block, then one U/V pair per block.
    for by in 0..h / 2 {
        for bx in 0..w / 2 {
            let (mut sb, mut sg, mut sr) = (0i32, 0i32, 0i32);
            for dy in 0..2 {
                for dx in 0..2 {
                    let s = (src_row(by * 2 + dy) * w + bx * 2 + dx) * 4;
                    sb += src[s] as i32;
                    sg += src[s + 1] as i32;
                    sr += src[s + 2] as i32;
                }
            }
            let (b, g, r) = (sb / 4, sg / 4, sr / 4);
            let o = by * w + bx * 2;
            uv_plane[o] = clamp8(((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128);
            uv_plane[o + 1] = clamp8(((112 * r - 94 * g - 18 * b + 128) >> 8) + 128);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bgrx_solid(w: usize, h: usize, b: u8, g: u8, r: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            v.extend_from_slice(&[b, g, r, 0xFF]);
        }
        v
    }

    #[test]
    fn nv12_gray_is_neutral() {
        let src = bgrx_solid(2, 2, 128, 128, 128);
        let mut dst = vec![0u8; 2 * 2 * 3 / 2];
        bgrx_to_nv12(&src, 2, 2, &mut dst, false);
        assert_eq!(&dst[..4], &[126, 126, 126, 126]); // Y
        assert_eq!(&dst[4..], &[128, 128]); // U=V=128 (achromatic)
    }

    #[test]
    fn nv12_white_and_black() {
        let mut dst = vec![0u8; 2 * 2 * 3 / 2];
        bgrx_to_nv12(&bgrx_solid(2, 2, 255, 255, 255), 2, 2, &mut dst, false);
        assert_eq!(&dst[..4], &[235, 235, 235, 235]);
        assert_eq!(&dst[4..], &[128, 128]);
        bgrx_to_nv12(&bgrx_solid(2, 2, 0, 0, 0), 2, 2, &mut dst, false);
        assert_eq!(&dst[..4], &[16, 16, 16, 16]);
        assert_eq!(&dst[4..], &[128, 128]);
    }

    #[test]
    fn nv12_pure_red_matches_bt601() {
        // BT.601 studio-swing red: Y≈82, U≈90, V≈240.
        let mut dst = vec![0u8; 2 * 2 * 3 / 2];
        bgrx_to_nv12(&bgrx_solid(2, 2, 0, 0, 255), 2, 2, &mut dst, false);
        assert_eq!(&dst[..4], &[82, 82, 82, 82]);
        assert_eq!(&dst[4..], &[90, 240]);
    }

    #[test]
    fn nv12_bottom_up_orientation() {
        // 2x2 image: top row black, bottom row white. Stored bottom-up
        // (RGB32 DIB order): memory row 0 = image bottom = white.
        let mut src = bgrx_solid(1, 1, 255, 255, 255);
        src.extend_from_slice(&[255, 255, 255, 0xFF]);
        src.extend_from_slice(&[0, 0, 0, 0xFF, 0, 0, 0, 0xFF]);
        let mut dst = vec![0u8; 2 * 2 * 3 / 2];
        bgrx_to_nv12(&src, 2, 2, &mut dst, true);
        assert_eq!(&dst[..4], &[16, 16, 235, 235]); // top-down: black row then white row
    }

    #[test]
    fn nv12_chroma_averages_block() {
        // 2x2: half red (0,0,255) half blue (255,0,0) → U/V between the two.
        let mut src = bgrx_solid(1, 1, 0, 0, 255);
        src.extend_from_slice(&[255, 0, 0, 0xFF, 0, 0, 255, 0xFF, 255, 0, 0, 0xFF]);
        let mut dst = vec![0u8; 2 * 2 * 3 / 2];
        bgrx_to_nv12(&src, 2, 2, &mut dst, false);
        let (u, v) = (dst[4], dst[5]);
        // Red alone would be U=90/V=240, blue U=240/V=110 — the block
        // average must land strictly between.
        assert!(u > 90 && u < 240, "u={u}");
        assert!(v > 110 && v < 240, "v={v}");
    }
}
