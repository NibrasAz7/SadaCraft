//! Mask-driven frame effects for `sadacam-cli feed` (stretch features).
//!
//! These consume the float person mask produced by [`crate::segment`]:
//!
//! * [`AutoFramer`] — `--auto-frame`: bounding-box the person in the mask,
//!   pad + fit the output aspect, EMA-smooth the crop box over time, and
//!   bilinear-rescale the cropped region to fill the WxH BGRX frame. The
//!   mask is rescaled with the same crop so later stages (background
//!   composite, relight) stay aligned with the framed pixels.
//! * [`relight`] — `--relight <0..1>`: a cheap "virtual key light" — each
//!   BGR channel is scaled by `1 + strength*(mask - 0.5)*RANGE`, so the
//!   person brightens (up to x1.4) while the background dims (down to
//!   x0.6). Equal channel scaling moves luma without shifting hue, and
//!   bilinear mask sampling keeps the transition soft at the edges.
//!
//! ## Eye-contact correction — investigated, deliberately not implemented
//!
//! Real eye contact (the speaker reads a teleprompter yet appears to look
//! into the lens) is a per-eye 3D gaze redirect, not a frame-wide filter.
//! A correct CPU implementation would need:
//!
//! 1. A face-landmark model with iris points (e.g. MediaPipe Face Mesh,
//!    478 landmarks) running every frame — a second ONNX model next to the
//!    selfie segmenter.
//! 2. Head pose from those landmarks (solvePnP) to estimate the gaze-to-
//!    camera offset vector.
//! 3. A per-eye warp: extract an eye patch per eye, then either flow-warp
//!    the iris/sclera toward the lens axis, or run a small generative
//!    redirect model (NVIDIA Maxine style — that path is GPU territory).
//! 4. Masked re-compositing of the warped patch so eyelids/edges do not
//!    smear.
//!
//! That is two extra model inferences plus per-eye warps per frame — not a
//! realistic fit for this CPU feed loop at 30 fps — and faking it with a
//! global transform looks visibly wrong, so no CLI flag exists for it.

use std::time::{Duration, Instant};

/// Frames without a detection before auto-frame eases back to full view.
const LOST_PERSON: Duration = Duration::from_millis(1000);
/// Zoom limiter: the crop never gets tighter than 1/MAX_ZOOM of the frame.
const MAX_ZOOM: f32 = 3.0;
/// Peak relight swing at strength=1: person x1.4, background x0.6.
const RELIGHT_RANGE: f32 = 0.8;

/// Normalized (0..1) rectangle stored as center + size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    const FULL: RectF = RectF {
        cx: 0.5,
        cy: 0.5,
        w: 1.0,
        h: 1.0,
    };

    fn lerp(self, to: RectF, a: f32) -> RectF {
        RectF {
            cx: self.cx + (to.cx - self.cx) * a,
            cy: self.cy + (to.cy - self.cy) * a,
            w: self.w + (to.w - self.w) * a,
            h: self.h + (to.h - self.h) * a,
        }
    }
}

/// Bounding box of mask pixels `>= thr`, normalized to 0..1.
/// `None` when no pixel clears the threshold.
pub fn person_box(mask: &[f32], mw: usize, mh: usize, thr: f32) -> Option<RectF> {
    let (mut x0, mut x1, mut y0, mut y1) = (mw, 0usize, mh, 0usize);
    let mut found = false;
    for (i, &m) in mask.iter().enumerate().take(mw * mh) {
        if m >= thr {
            let (x, y) = (i % mw, i / mw);
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
            found = true;
        }
    }
    if !found {
        return None;
    }
    Some(RectF {
        cx: (x0 + x1 + 1) as f32 * 0.5 / mw as f32,
        cy: (y0 + y1 + 1) as f32 * 0.5 / mh as f32,
        w: (x1 + 1 - x0) as f32 / mw as f32,
        h: (y1 + 1 - y0) as f32 / mh as f32,
    })
}

/// Add `frac` padding around `b` (each dimension), clamped to the frame.
fn pad(b: RectF, frac: f32) -> RectF {
    RectF {
        cx: b.cx,
        cy: b.cy,
        w: (b.w * (1.0 + frac)).min(1.0),
        h: (b.h * (1.0 + frac)).min(1.0),
    }
}

/// Expand `b` to the output aspect `frame_aspect` (w/h), apply the
/// `MAX_ZOOM` limit on the crop height, then clamp the center so the box
/// stays fully inside the frame. Best-effort: a box that cannot both fit
/// the aspect and stay in-frame is shrunk to the frame edge.
fn fit(b: RectF, frame_aspect: f32) -> RectF {
    let (mut w, mut h) = (b.w, b.h);
    if w / h < frame_aspect {
        w = h * frame_aspect;
    } else {
        h = w / frame_aspect;
    }
    if h < 1.0 / MAX_ZOOM {
        h = 1.0 / MAX_ZOOM;
        w = h * frame_aspect;
    }
    w = w.min(1.0);
    h = h.min(1.0);
    RectF {
        cx: b.cx.clamp(w * 0.5, 1.0 - w * 0.5),
        cy: b.cy.clamp(h * 0.5, 1.0 - h * 0.5),
        w,
        h,
    }
}

/// Temporal smoother + cropper behind `--auto-frame`.
///
/// Each frame [`person_box`] yields a raw person rectangle; [`update`]
/// EMA-blends it into a jitter-free crop and [`apply`] rescales the frame
/// (and the mask) through it. When the person vanishes, the last crop is
/// held for [`LOST_PERSON`], then the box eases back to the full frame.
pub struct AutoFramer {
    /// EMA weight of each new observation (0..1; smaller = smoother).
    pub alpha: f32,
    /// Padding added around the person box (fraction of box size).
    pub margin: f32,
    /// Mask probability counted as "person".
    pub threshold: f32,
    state: RectF,
    last_person: Option<Instant>,
    scratch: Vec<u8>,
    mask_scratch: Vec<f32>,
}

impl AutoFramer {
    pub fn new() -> Self {
        AutoFramer {
            alpha: 0.1,
            margin: 0.4,
            threshold: 0.5,
            state: RectF::FULL,
            last_person: None,
            scratch: Vec::new(),
            mask_scratch: Vec::new(),
        }
    }

    /// Advance the smoother one frame; returns the smoothed crop to apply.
    pub fn update(&mut self, det: Option<RectF>) -> RectF {
        self.update_at(det, Instant::now())
    }

    /// `Instant`-injectable core so tests control the fallback timer.
    pub fn update_at(&mut self, det: Option<RectF>, now: Instant) -> RectF {
        let target = match det {
            Some(b) => {
                self.last_person = Some(now);
                pad(b, self.margin)
            }
            None => match self.last_person {
                // Recent dropout: hold the last smoothed crop.
                Some(t) if now.duration_since(t) < LOST_PERSON => self.state,
                _ => RectF::FULL,
            },
        };
        self.state = self.state.lerp(target, self.alpha);
        self.state
    }

    /// Run one frame: find the person in `mask` (if any), smooth the crop,
    /// rescale the cropped region to fill `frame`, and rescale `mask` the
    /// same way so downstream mask consumers (background composite,
    /// relight) stay aligned. `mask` may be `None` — the frame still gets
    /// the smoothed fallback crop (full frame once the hold expires).
    pub fn apply(
        &mut self,
        frame: &mut [u8],
        w: usize,
        h: usize,
        mask: Option<(&mut Vec<f32>, usize, usize)>,
    ) {
        let det = mask
            .as_ref()
            .and_then(|(m, mw, mh)| person_box(m.as_slice(), *mw, *mh, self.threshold));
        let crop = fit(self.update(det), w as f32 / h as f32);
        // Whole-frame crop (the usual no-person case): nothing to do.
        if crop.w >= 0.999 && crop.h >= 0.999 {
            return;
        }
        crop_rescale_bgrx_inplace(frame, w, h, crop, &mut self.scratch);
        if let Some((m, mw, mh)) = mask {
            crop_rescale_f32_inplace(m.as_mut_slice(), mw, mh, crop, &mut self.mask_scratch);
        }
    }
}

/// Bilinear-rescale the normalized `crop` region of `frame` over the whole
/// WxH buffer. `scratch` holds the source copy; reads go through it so the
/// write is in-place safe.
fn crop_rescale_bgrx_inplace(
    frame: &mut [u8],
    w: usize,
    h: usize,
    crop: RectF,
    scratch: &mut Vec<u8>,
) {
    scratch.clear();
    scratch.extend_from_slice(frame);
    let src: &[u8] = scratch;
    let sx = (crop.cx - crop.w * 0.5) * w as f32;
    let sy = (crop.cy - crop.h * 0.5) * h as f32;
    let (sw, sh) = (crop.w * w as f32, crop.h * h as f32);
    for y in 0..h {
        let fy = (sy + (y as f32 + 0.5) * sh / h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(h - 1);
        let ty = fy.fract();
        for x in 0..w {
            let fx = (sx + (x as f32 + 0.5) * sw / w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(w - 1);
            let tx = fx.fract();
            let d = (y * w + x) * 4;
            let (s00, s01, s10, s11) =
                ((y0 * w + x0) * 4, (y0 * w + x1) * 4, (y1 * w + x0) * 4, (y1 * w + x1) * 4);
            for c in 0..4 {
                let v = src[s00 + c] as f32 * (1.0 - tx) * (1.0 - ty)
                    + src[s01 + c] as f32 * tx * (1.0 - ty)
                    + src[s10 + c] as f32 * (1.0 - tx) * ty
                    + src[s11 + c] as f32 * tx * ty;
                frame[d + c] = v as u8;
            }
        }
    }
}

/// Same crop+rescale as [`crop_rescale_bgrx_inplace`] for a single-channel
/// float mask (kept at model resolution — cheap, and downstream consumers
/// already bilinear-sample it up to frame res).
fn crop_rescale_f32_inplace(
    mask: &mut [f32],
    mw: usize,
    mh: usize,
    crop: RectF,
    scratch: &mut Vec<f32>,
) {
    scratch.clear();
    scratch.extend_from_slice(mask);
    let src: &[f32] = scratch;
    let sx = (crop.cx - crop.w * 0.5) * mw as f32;
    let sy = (crop.cy - crop.h * 0.5) * mh as f32;
    let (sw, sh) = (crop.w * mw as f32, crop.h * mh as f32);
    for y in 0..mh {
        let fy = (sy + (y as f32 + 0.5) * sh / mh as f32 - 0.5).clamp(0.0, (mh - 1) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(mh - 1);
        let ty = fy.fract();
        for x in 0..mw {
            let fx = (sx + (x as f32 + 0.5) * sw / mw as f32 - 0.5).clamp(0.0, (mw - 1) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(mw - 1);
            let tx = fx.fract();
            mask[y * mw + x] = src[y0 * mw + x0] * (1.0 - tx) * (1.0 - ty)
                + src[y0 * mw + x1] * tx * (1.0 - ty)
                + src[y1 * mw + x0] * (1.0 - tx) * ty
                + src[y1 * mw + x1] * tx * ty;
        }
    }
}

/// Per-channel relight scale for mask value `mask` at `strength` (0..1):
/// 1.0 at mask=0.5, up to `1 + strength*RANGE/2` for the person, down to
/// `1 - strength*RANGE/2` for the background.
pub fn relight_scale(mask: f32, strength: f32) -> f32 {
    (1.0 + strength * (mask - 0.5) * RELIGHT_RANGE).max(0.0)
}

/// Mask-guided relight, in place on the BGRX frame: the person region is
/// brightened and the background dimmed proportionally to `strength`.
/// Row-wise bilinear mask sampling (same pattern as the background
/// composite); ~a handful of flops per pixel — well under 3 ms at 720p.
pub fn relight(
    frame: &mut [u8],
    w: usize,
    h: usize,
    mask: &[f32],
    mw: usize,
    mh: usize,
    strength: f32,
) {
    if strength <= 0.0 || mask.len() < mw * mh || mw == 0 || mh == 0 {
        return;
    }
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * mh as f32 / h as f32 - 0.5).clamp(0.0, (mh - 1) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(mh - 1);
        let ty = fy.fract();
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * mw as f32 / w as f32 - 0.5).clamp(0.0, (mw - 1) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(mw - 1);
            let tx = fx.fract();
            let m = (mask[y0 * mw + x0] * (1.0 - tx) * (1.0 - ty)
                + mask[y0 * mw + x1] * tx * (1.0 - ty)
                + mask[y1 * mw + x0] * (1.0 - tx) * ty
                + mask[y1 * mw + x1] * tx * ty)
                .clamp(0.0, 1.0);
            let s = relight_scale(m, strength);
            let d = (y * w + x) * 4;
            frame[d] = (frame[d] as f32 * s + 0.5).min(255.0) as u8;
            frame[d + 1] = (frame[d + 1] as f32 * s + 0.5).min(255.0) as u8;
            frame[d + 2] = (frame[d + 2] as f32 * s + 0.5).min(255.0) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(cx: f32, cy: f32, w: f32, h: f32) -> Option<RectF> {
        Some(RectF { cx, cy, w, h })
    }

    #[test]
    fn person_box_bounds() {
        // 8x8 mask; person pixels in rows 2..=4, cols 3..=5.
        let mut m = vec![0.1f32; 64];
        for y in 2..=4usize {
            for x in 3..=5usize {
                m[y * 8 + x] = 0.9;
            }
        }
        let b = person_box(&m, 8, 8, 0.5).unwrap();
        assert!((b.w - 3.0 / 8.0).abs() < 1e-6);
        assert!((b.h - 3.0 / 8.0).abs() < 1e-6);
        assert!((b.cx - 4.5 / 8.0).abs() < 1e-6);
        assert!((b.cy - 3.5 / 8.0).abs() < 1e-6);
        assert!(person_box(&vec![0.0f32; 64], 8, 8, 0.5).is_none());
    }

    #[test]
    fn ema_converges_and_smooths_jitter() {
        let mut af = AutoFramer::new();
        let t0 = Instant::now();
        let target = pad(RectF { cx: 0.3, cy: 0.4, w: 0.2, h: 0.4 }, af.margin);
        // First step from the full frame toward the target by alpha.
        let s1 = af.update_at(det(0.3, 0.4, 0.2, 0.4), t0);
        assert!((s1.cx - (0.5 + af.alpha * (target.cx - 0.5))).abs() < 1e-6);
        // Repeated identical observations converge on the padded target.
        let mut s = s1;
        for i in 1..200u64 {
            s = af.update_at(
                det(0.3, 0.4, 0.2, 0.4),
                t0 + Duration::from_millis(33 * i),
            );
        }
        assert!((s.cx - target.cx).abs() < 1e-3);
        assert!((s.w - target.w).abs() < 1e-3);
        // A one-frame jump moves the smoothed box by ~alpha*delta, not the
        // full delta: raw cx would be 0.8, smoothed stays near 0.3.
        let j = af.update_at(det(0.8, 0.4, 0.2, 0.4), t0 + Duration::from_secs(7));
        assert!(j.cx < 0.5 && j.cx > target.cx, "cx={}", j.cx);
    }

    #[test]
    fn ema_holds_then_falls_back_to_full() {
        let mut af = AutoFramer::new();
        let t0 = Instant::now();
        let s = af.update_at(det(0.5, 0.5, 0.3, 0.6), t0);
        // Dropout < 1s: holds the smoothed crop exactly.
        let held = af.update_at(None, t0 + Duration::from_millis(500));
        assert_eq!(held, s);
        // Dropout > 1s: eases back toward the full frame.
        let mut back = af.update_at(None, t0 + Duration::from_secs(2));
        assert!(back.w > s.w);
        for i in 0..300u64 {
            back = af.update_at(
                None,
                t0 + Duration::from_secs(2) + Duration::from_millis(33 * i),
            );
        }
        assert!((back.w - 1.0).abs() < 1e-2, "w={}", back.w);
    }

    #[test]
    fn fit_matches_aspect_and_limits_zoom() {
        // Portrait person box on a 16:9 frame expands to the frame aspect.
        let r = fit(RectF { cx: 0.5, cy: 0.5, w: 0.2, h: 0.5 }, 16.0 / 9.0);
        assert!((r.w / r.h - 16.0 / 9.0).abs() < 1e-5);
        // Tiny detections are zoom-limited to 1/MAX_ZOOM vertically.
        let r = fit(RectF { cx: 0.5, cy: 0.5, w: 0.05, h: 0.05 }, 1.0);
        assert!((r.h - 1.0 / MAX_ZOOM).abs() < 1e-6);
        // Off-center boxes are pushed back inside the frame.
        let r = fit(RectF { cx: 0.95, cy: 0.5, w: 0.4, h: 0.4 }, 1.0);
        assert!(r.cx + r.w * 0.5 <= 1.0 + 1e-6, "cx={} w={}", r.cx, r.w);
    }

    #[test]
    fn crop_rescale_zooms_region() {
        // 4x4 BGRX, left half black / right half white; crop the right half.
        let (w, h) = (4usize, 4usize);
        let mut f = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let d = (y * w + x) * 4;
                let v = if x >= 2 { 255u8 } else { 0 };
                f[d] = v;
                f[d + 1] = v;
                f[d + 2] = v;
                f[d + 3] = 0xFF;
            }
        }
        let mut sc = Vec::new();
        crop_rescale_bgrx_inplace(
            &mut f,
            w,
            h,
            RectF { cx: 0.75, cy: 0.5, w: 0.5, h: 1.0 },
            &mut sc,
        );
        // Interior columns land fully inside the white region.
        assert_eq!(f[(2 * w + 2) * 4], 255);
        // The leftmost output column samples just across the seam (~191).
        assert!(f[(2 * w) * 4] > 150, "v={}", f[(2 * w) * 4]);
    }

    #[test]
    fn autoframe_empty_mask_is_identity() {
        let mut af = AutoFramer::new();
        let (w, h) = (4usize, 4usize);
        let orig = vec![7u8; w * h * 4];
        let mut f = orig.clone();
        let mut m = vec![0f32; 16];
        af.apply(&mut f, w, h, Some((&mut m, 4, 4)));
        assert_eq!(f, orig);
    }

    #[test]
    fn autoframe_crops_frame_and_keeps_mask_aligned() {
        let mut af = AutoFramer::new();
        let (w, h) = (8usize, 8usize);
        let (mw, mh) = (4usize, 4usize);
        // Person = center-right quarter of the mask (cols 2..4, rows 1..3).
        let mut m0 = vec![0f32; mw * mh];
        for y in 1..3usize {
            for x in 2..4usize {
                m0[y * mw + x] = 1.0;
            }
        }
        let mut f = vec![0u8; w * h * 4];
        // Production hands `apply` a fresh inference mask each frame; the
        // test mirrors that or the in-place rescale feeds back into the
        // detector and the crop degenerates to full frame.
        let mut m = m0.clone();
        for _ in 0..300 {
            // Same scene each frame: right half bright.
            for y in 0..h {
                for x in 0..w {
                    let d = (y * w + x) * 4;
                    let v = if x >= w / 2 { 255u8 } else { 0 };
                    f[d] = v;
                    f[d + 1] = v;
                    f[d + 2] = v;
                    f[d + 3] = 0xFF;
                }
            }
            m = m0.clone();
            af.apply(&mut f, w, h, Some((&mut m, mw, mh)));
        }
        // After convergence the crop zoomed onto the bright person region,
        // so the framed output is mostly bright.
        let mean: u32 = f.iter().step_by(4).map(|&b| b as u32).sum::<u32>() / (w * h) as u32;
        assert!(mean > 150, "mean={mean}");
        // The mask was rescaled through the same crop and still peaks at ~1.
        let peak = m.iter().copied().fold(0f32, f32::max);
        assert!(peak > 0.9, "peak={peak}");
    }

    #[test]
    fn relight_scale_curve() {
        assert!((relight_scale(0.5, 1.0) - 1.0).abs() < 1e-6);
        assert!((relight_scale(1.0, 1.0) - 1.4).abs() < 1e-6);
        assert!((relight_scale(0.0, 1.0) - 0.6).abs() < 1e-6);
        assert!((relight_scale(1.0, 0.0) - 1.0).abs() < 1e-6);
        // Never negative even if fed an out-of-range strength.
        assert!(relight_scale(0.0, 5.0) >= 0.0);
    }

    #[test]
    fn relight_brightens_person_dims_background() {
        let (w, h, mw, mh) = (2usize, 2usize, 2usize, 2usize);
        let mut f = vec![0u8; w * h * 4];
        for i in 0..w * h {
            f[i * 4] = 100;
            f[i * 4 + 1] = 100;
            f[i * 4 + 2] = 100;
            f[i * 4 + 3] = 0xFF;
        }
        // Person at (0,0) and (0,1) in row-major mask order.
        let mask = vec![1.0f32, 0.0, 1.0, 0.0];
        relight(&mut f, w, h, &mask, mw, mh, 1.0);
        assert_eq!(f[0], 140); // (0,0) person x1.4
        assert_eq!(f[(0 * w + 1) * 4], 60); // (1,0) background x0.6
        assert_eq!(f[(1 * w) * 4], 140);
        assert_eq!(f[(1 * w + 1) * 4], 60);
        assert_eq!(f[3], 0xFF); // alpha untouched
    }

    #[test]
    fn relight_zero_strength_is_noop() {
        let mut f = vec![50u8; 16];
        let orig = f.clone();
        relight(&mut f, 2, 2, &[1.0f32; 4], 2, 2, 0.0);
        assert_eq!(f, orig);
    }
}
