//! Pure-Rust effect engine.
//!
//! Holds effect state and renders premultiplied-BGRA frames for the layered
//! overlay window. No Win32 dependency — the engine can be driven by the
//! overlay's message loop now and by the Tauri app later.
//!
//! Coordinate space is *buffer-local*: (0,0) is the top-left of the virtual
//! screen rect; the caller converts screen coords by subtracting the origin.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::config::Config;

const MAX_RIPPLES: usize = 32;
const MAX_TRAIL: usize = 512;
const MAX_KEYS: usize = 10;
/// Antialiased edge width for stamped shapes, in pixels.
const AA: f32 = 1.5;

#[derive(Clone, Copy)]
struct Bounds {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

struct Ripple {
    x: f32,
    y: f32,
    start: Instant,
}

struct TrailPoint {
    x: f32,
    y: f32,
    at: Instant,
}

/// Rasterized glyph: row-major alpha mask (`w*h` bytes), pre-rendered by the
/// platform layer (GDI) so the engine stays Win32-free.
pub struct Glyph {
    pub w: u8,
    pub h: u8,
    pub mask: Vec<u8>,
}

struct KeyChip {
    label: String,
    at: Instant,
}

pub struct Engine {
    pub cfg: Config,
    w: i32,
    h: i32,
    cursor: (f32, f32),
    ripples: Vec<Ripple>,
    trail: VecDeque<TrailPoint>,
    /// Persistent premultiplied-BGRA ink layer (annotation pen strokes).
    ink: Vec<u8>,
    /// Union of all ink stamps — bounds the per-frame ink composite.
    ink_bounds: Option<Bounds>,
    pen_down: bool,
    pen_last: (f32, f32),
    pen_color: usize,
    /// Recent keystrokes shown as fading chips (keystroke viz).
    key_chips: Vec<KeyChip>,
    /// Glyph atlas for chip text; populated by `set_glyphs`.
    glyphs: HashMap<char, Glyph>,
    /// Uniform cell height for atlas glyphs.
    glyph_h: f32,
    /// Primary-monitor rect in buffer-local coords — key chips anchor to its
    /// bottom edge so they land on the right monitor in multi-screen setups.
    key_area: (f32, f32, f32, f32),
    dirty: bool,
}

impl Engine {
    pub fn new(cfg: Config, w: i32, h: i32) -> Self {
        Self {
            cfg,
            w,
            h,
            cursor: (0.0, 0.0),
            ripples: Vec::new(),
            trail: VecDeque::new(),
            ink: vec![0u8; (w.max(0) * h.max(0)) as usize * 4],
            ink_bounds: None,
            pen_down: false,
            pen_last: (0.0, 0.0),
            pen_color: 0,
            key_chips: Vec::new(),
            glyphs: HashMap::new(),
            glyph_h: 0.0,
            key_area: (0.0, 0.0, w as f32, h as f32),
            dirty: true,
        }
    }

    /// Reallocate for a new virtual-screen size (drops ink).
    pub fn resize(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
        self.ink = vec![0u8; (w.max(0) * h.max(0)) as usize * 4];
        self.ink_bounds = None;
        self.trail.clear();
        self.dirty = true;
    }

    /// Update the cursor (buffer-local coords). Returns whether any
    /// cursor-following effect state changed (i.e. a repaint is needed).
    pub fn move_cursor(&mut self, x: f32, y: f32, now: Instant) -> bool {
        let moved =
            (self.cursor.0 - x).abs() > 0.5 || (self.cursor.1 - y).abs() > 0.5;
        self.cursor = (x, y);
        let mut changed =
            moved && (self.cfg.ring.enabled || self.cfg.spotlight.enabled);
        if self.cfg.trail.enabled {
            let far = self
                .trail
                .back()
                .map_or(true, |p| (p.x - x).hypot(p.y - y) >= 2.0);
            if far {
                self.trail.push_back(TrailPoint { x, y, at: now });
                if self.trail.len() > MAX_TRAIL {
                    self.trail.pop_front();
                }
                changed = true;
            }
        }
        changed
    }

    /// Spawn a click ripple (buffer-local coords).
    pub fn click(&mut self, x: f32, y: f32, now: Instant) {
        if !self.cfg.ripple.enabled {
            return;
        }
        if self.ripples.len() >= MAX_RIPPLES {
            self.ripples.remove(0);
        }
        self.ripples.push(Ripple { x, y, start: now });
        self.dirty = true;
    }

    // --- annotation pen -------------------------------------------------

    pub fn pen_begin(&mut self, x: f32, y: f32) {
        self.pen_down = true;
        self.pen_last = (x, y);
        self.stamp_ink(x, y);
    }

    pub fn pen_move(&mut self, x: f32, y: f32) {
        if !self.pen_down {
            return;
        }
        // Walk the segment stamping overlapping discs for a smooth stroke.
        let (lx, ly) = self.pen_last;
        let d = (x - lx).hypot(y - ly);
        let step = (self.pen_width() / 3.0).max(1.0);
        let n = (d / step).ceil() as i32;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            self.stamp_ink(lx + (x - lx) * t, ly + (y - ly) * t);
        }
        self.pen_last = (x, y);
    }

    pub fn pen_end(&mut self) {
        self.pen_down = false;
    }

    pub fn set_pen_color(&mut self, idx: usize) {
        if idx < self.cfg.pen.colors.len() {
            self.pen_color = idx;
        }
    }

    pub fn clear_ink(&mut self) {
        self.ink.fill(0);
        self.ink_bounds = None;
        self.dirty = true;
    }

    // --- keystroke visualization -----------------------------------------

    /// Install the platform-rasterized glyph atlas (uniform cell height).
    pub fn set_glyphs(&mut self, glyphs: Vec<(char, u8, u8, Vec<u8>)>) {
        self.glyph_h = glyphs.iter().map(|g| g.2 as f32).fold(0.0, f32::max);
        self.glyphs = glyphs
            .into_iter()
            .map(|(c, w, h, mask)| (c, Glyph { w, h, mask }))
            .collect();
        self.dirty = true;
    }

    /// Set the primary-monitor rect (buffer-local coords) for chip anchoring.
    pub fn set_key_area(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.key_area = (x0, y0, x1, y1);
        self.dirty = true;
    }

    /// Record a key press; a chip with `label` fades in/out at the anchor.
    pub fn key_press(&mut self, label: String, now: Instant) {
        if !self.cfg.keys.enabled {
            return;
        }
        // Merge a repeat of the newest chip instead of piling up duplicates.
        if let Some(last) = self.key_chips.last_mut() {
            if last.label == label {
                last.at = now;
                self.dirty = true;
                return;
            }
        }
        if self.key_chips.len() >= MAX_KEYS {
            self.key_chips.remove(0);
        }
        self.key_chips.push(KeyChip { label, at: now });
        self.dirty = true;
    }

    // --- frame bookkeeping ----------------------------------------------

    /// True while transient effects need continuous repainting.
    pub fn animated(&self) -> bool {
        !self.ripples.is_empty() || !self.trail.is_empty() || !self.key_chips.is_empty()
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }

    /// Evict fully-decayed ripples / trail points.
    pub fn prune(&mut self, now: Instant) {
        let dur = Duration::from_millis(self.cfg.ripple.duration_ms.max(1));
        self.ripples.retain(|r| now.duration_since(r.start) < dur);
        let life = Duration::from_millis(self.cfg.trail.lifetime_ms.max(1));
        while let Some(p) = self.trail.front() {
            if now.duration_since(p.at) >= life {
                self.trail.pop_front();
            } else {
                break;
            }
        }
        let klife = Duration::from_millis(self.cfg.keys.lifetime_ms.max(1));
        self.key_chips.retain(|k| now.duration_since(k.at) < klife);
    }

    // --- rendering --------------------------------------------------------

    /// Render the frame into `buf` (premultiplied BGRA, `w*h*4` bytes).
    pub fn render(&mut self, buf: &mut [u8], now: Instant) {
        debug_assert_eq!(buf.len(), (self.w * self.h * 4) as usize);
        buf.fill(0);
        self.render_spotlight(buf);
        self.render_trail(buf, now);
        self.render_ring(buf);
        self.render_ripples(buf, now);
        self.composite_ink(buf);
        self.render_keys(buf, now);
        self.prune(now);
    }

    /// Darken everything except a feathered circle around the cursor.
    fn render_spotlight(&self, buf: &mut [u8]) {
        let c = &self.cfg.spotlight;
        if !c.enabled {
            return;
        }
        let dim = (c.dim.clamp(0.0, 1.0) * 255.0).round() as u8;
        for px in buf.chunks_exact_mut(4) {
            px[3] = dim; // RGB already 0 -> premultiplied black at `dim`.
        }
        let (cx, cy) = self.cursor;
        let r = c.radius.max(1.0);
        let f = c.feather.max(1.0);
        let (x0, y0, x1, y1) = clip(cx - r - f, cy - r - f, cx + r + f, cy + r + f, self.w, self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = (x as f32 - cx).hypot(y as f32 - cy);
                // k: 0 inside the clear radius -> 1 beyond the feather edge.
                let k = ((d - r) / f).clamp(0.0, 1.0);
                if k < 1.0 {
                    let i = (y * self.w + x) as usize * 4;
                    buf[i + 3] = (dim as f32 * k).round() as u8;
                }
            }
        }
    }

    fn render_trail(&self, buf: &mut [u8], now: Instant) {
        let c = &self.cfg.trail;
        if !c.enabled {
            return;
        }
        let life = c.lifetime_ms.max(1) as f32;
        for p in &self.trail {
            let age = now.duration_since(p.at).as_secs_f32() * 1000.0;
            let a = c.opacity * (1.0 - age / life).clamp(0.0, 1.0);
            if a > 0.01 {
                stamp_disc(buf, self.w, self.h, p.x, p.y, c.radius, c.color, a);
            }
        }
    }

    fn render_ring(&self, buf: &mut [u8]) {
        let c = &self.cfg.ring;
        if !c.enabled {
            return;
        }
        stamp_ring(buf, self.w, self.h, self.cursor.0, self.cursor.1, c.radius, c.thickness, c.color, c.opacity);
    }

    fn render_ripples(&self, buf: &mut [u8], now: Instant) {
        let c = &self.cfg.ripple;
        if !c.enabled {
            return;
        }
        let dur = c.duration_ms.max(1) as f32;
        for r in &self.ripples {
            let t = (now.duration_since(r.start).as_secs_f32() * 1000.0 / dur).clamp(0.0, 1.0);
            let ease = 1.0 - (1.0 - t) * (1.0 - t);
            let a = c.opacity * (1.0 - t);
            if a > 0.01 {
                stamp_ring(buf, self.w, self.h, r.x, r.y, c.max_radius * ease, c.thickness, c.color, a);
            }
        }
    }

    /// src-over composite the persistent ink layer over the frame,
    /// restricted to the ink bounding box.
    fn composite_ink(&self, buf: &mut [u8]) {
        let Some(b) = self.ink_bounds else { return };
        let w = self.w;
        for y in b.y0.max(0)..b.y1.min(self.h) {
            let row = (y * w) as usize * 4;
            for x in b.x0.max(0)..b.x1.min(w) {
                let i = row + x as usize * 4;
                let sa = self.ink[i + 3] as u16;
                if sa == 0 {
                    continue;
                }
                let inv = 255 - sa;
                for c in 0..3 {
                    buf[i + c] =
                        (self.ink[i + c] as u16 + (buf[i + c] as u16 * inv + 127) / 255).min(255) as u8;
                }
                buf[i + 3] = (sa + (buf[i + 3] as u16 * inv + 127) / 255).min(255) as u8;
            }
        }
    }

    /// Fading key chips in a single centered row above the bottom edge of
    /// the primary monitor (`key_area`).
    fn render_keys(&self, buf: &mut [u8], now: Instant) {
        let c = &self.cfg.keys;
        if !c.enabled || self.key_chips.is_empty() || self.glyph_h <= 0.0 {
            return;
        }
        const PAD_X: f32 = 8.0;
        const PAD_Y: f32 = 4.0;
        const GAP: f32 = 6.0;
        const RADIUS: f32 = 6.0;
        const FADE_IN_MS: f32 = 80.0;

        let life = c.lifetime_ms.max(1) as f32;
        let chip_h = self.glyph_h + PAD_Y * 2.0;

        // Per-chip: measured text width and current alpha (fade in/out).
        let chip_w = |label: &str| -> f32 {
            let text_w: f32 = label
                .chars()
                .map(|ch| self.glyph(ch).map_or(self.glyph_h * 0.6, |g| g.w as f32) + 1.0)
                .sum::<f32>()
                .max(0.0)
                - 1.0;
            text_w.max(1.0) + PAD_X * 2.0
        };
        let chips: Vec<(&KeyChip, f32, f32)> = self
            .key_chips
            .iter()
            .map(|k| {
                let age = now.duration_since(k.at).as_secs_f32() * 1000.0;
                let fade_in = (age / FADE_IN_MS).min(1.0);
                let fade_out = (1.0 - age / life).clamp(0.0, 1.0);
                (k, chip_w(&k.label), fade_in * fade_out * c.opacity)
            })
            .collect();

        let total: f32 = chips.iter().map(|(_, w, _)| w).sum::<f32>() + GAP * (chips.len() - 1) as f32;
        let (ax0, _ay0, ax1, ay1) = self.key_area;
        let mut x = ((ax0 + ax1) / 2.0 - total / 2.0).round();
        let y = (ay1 - c.margin_bottom - chip_h).round();

        for (k, w, a) in chips {
            if a <= 0.01 {
                x += w + GAP;
                continue;
            }
            stamp_round_rect(buf, self.w, self.h, x, y, w, chip_h, RADIUS, c.bg, a * 0.85);
            let mut gx = x + PAD_X;
            let gy = y + PAD_Y;
            for ch in k.label.chars() {
                match self.glyph(ch) {
                    Some(g) => {
                        stamp_glyph(buf, self.w, self.h, gx, gy, g, c.color, a);
                        gx += g.w as f32 + 1.0;
                    }
                    None => gx += self.glyph_h * 0.6 + 1.0,
                }
            }
            x += w + GAP;
        }
    }

    fn glyph(&self, ch: char) -> Option<&Glyph> {
        self.glyphs.get(&ch).or_else(|| self.glyphs.get(&'?'))
    }

    // --- pen internals ----------------------------------------------------

    fn pen_width(&self) -> f32 {
        self.cfg.pen.width.clamp(1.0, 60.0)
    }

    fn pen_rgb(&self) -> [u8; 3] {
        self.cfg
            .pen
            .colors
            .get(self.pen_color)
            .copied()
            .unwrap_or([255, 60, 50])
    }

    /// Stamp one pen disc into the persistent ink layer and grow `ink_bounds`.
    fn stamp_ink(&mut self, x: f32, y: f32) {
        let r = self.pen_width() / 2.0;
        let [cr, cg, cb] = self.pen_rgb();
        let (x0, y0, x1, y1) =
            clip(x - r - AA, y - r - AA, x + r + AA, y + r + AA, self.w, self.h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for py in y0..y1 {
            for px in x0..x1 {
                let d = (px as f32 - x).hypot(py as f32 - y);
                let v = ((r + AA - d) / AA).min(1.0);
                if v <= 0.0 {
                    continue;
                }
                let a = (v * 255.0).round() as u16;
                let inv = 255 - a;
                let i = (py * self.w + px) as usize * 4;
                let ink = &mut self.ink[i..i + 4];
                ink[0] = ((cb as u16 * a + 127) / 255 + (ink[0] as u16 * inv + 127) / 255).min(255) as u8;
                ink[1] = ((cg as u16 * a + 127) / 255 + (ink[1] as u16 * inv + 127) / 255).min(255) as u8;
                ink[2] = ((cr as u16 * a + 127) / 255 + (ink[2] as u16 * inv + 127) / 255).min(255) as u8;
                ink[3] = (a + (ink[3] as u16 * inv + 127) / 255).min(255) as u8;
            }
        }
        let b = self.ink_bounds.get_or_insert(Bounds { x0, y0, x1, y1 });
        b.x0 = b.x0.min(x0);
        b.y0 = b.y0.min(y0);
        b.x1 = b.x1.max(x1);
        b.y1 = b.y1.max(y1);
        self.dirty = true;
    }
}

// --- pixel helpers ---------------------------------------------------------

fn clip(x0: f32, y0: f32, x1: f32, y1: f32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    (
        (x0.floor() as i32).clamp(0, w),
        (y0.floor() as i32).clamp(0, h),
        (x1.ceil() as i32).clamp(0, w),
        (y1.ceil() as i32).clamp(0, h),
    )
}

/// src-over blend of a source color at `a` (0-255) into a premultiplied
/// BGRA pixel. `dst` is a 4-byte pixel slice.
#[inline]
fn over(dst: &mut [u8], r: u8, g: u8, b: u8, a: u16) {
    let inv = 255 - a;
    dst[0] = ((b as u16 * a + 127) / 255 + (dst[0] as u16 * inv + 127) / 255).min(255) as u8;
    dst[1] = ((g as u16 * a + 127) / 255 + (dst[1] as u16 * inv + 127) / 255).min(255) as u8;
    dst[2] = ((r as u16 * a + 127) / 255 + (dst[2] as u16 * inv + 127) / 255).min(255) as u8;
    dst[3] = (a + (dst[3] as u16 * inv + 127) / 255).min(255) as u8;
}

/// Soft-edged filled disc. `alpha` is 0.0-1.0.
fn stamp_disc(buf: &mut [u8], w: i32, h: i32, cx: f32, cy: f32, r: f32, color: [u8; 3], alpha: f32) {
    let (x0, y0, x1, y1) = clip(cx - r - AA, cy - r - AA, cx + r + AA, cy + r + AA, w, h);
    let a255 = alpha.clamp(0.0, 1.0) * 255.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let d = (x as f32 - cx).hypot(y as f32 - cy);
            let v = ((r + AA - d) / AA).min(1.0);
            if v <= 0.0 {
                continue;
            }
            let i = (y * w + x) as usize * 4;
            over(&mut buf[i..i + 4], color[0], color[1], color[2], (v * a255).round() as u16);
        }
    }
}

/// Soft-edged annulus (ring). `alpha` is 0.0-1.0.
fn stamp_ring(
    buf: &mut [u8], w: i32, h: i32,
    cx: f32, cy: f32, radius: f32, thickness: f32,
    color: [u8; 3], alpha: f32,
) {
    let half = (thickness / 2.0).max(0.5);
    let outer = radius + half + AA;
    let (x0, y0, x1, y1) = clip(cx - outer, cy - outer, cx + outer, cy + outer, w, h);
    let a255 = alpha.clamp(0.0, 1.0) * 255.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let d = (x as f32 - cx).hypot(y as f32 - cy);
            let v = ((half + AA - (d - radius).abs()) / AA).min(1.0);
            if v <= 0.0 {
                continue;
            }
            let i = (y * w + x) as usize * 4;
            over(&mut buf[i..i + 4], color[0], color[1], color[2], (v * a255).round() as u16);
        }
    }
}

/// Filled rounded rectangle, top-left anchored. `alpha` is 0.0-1.0.
fn stamp_round_rect(
    buf: &mut [u8], w: i32, h: i32,
    x: f32, y: f32, rw: f32, rh: f32, rad: f32,
    color: [u8; 3], alpha: f32,
) {
    let (x0, y0, x1, y1) = clip(x - AA, y - AA, x + rw + AA, y + rh + AA, w, h);
    let a255 = alpha.clamp(0.0, 1.0) * 255.0;
    let (cx, cy) = (x + rw / 2.0, y + rh / 2.0);
    let (hx, hy) = ((rw / 2.0 - rad).max(0.0), (rh / 2.0 - rad).max(0.0));
    for py in y0..y1 {
        for px in x0..x1 {
            // Rounded-rect SDF: distance to the edge (negative inside).
            let qx = (px as f32 - cx).abs() - hx;
            let qy = (py as f32 - cy).abs() - hy;
            let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rad;
            let v = (AA - d).clamp(0.0, 1.0);
            if v <= 0.0 {
                continue;
            }
            let i = (py * w + px) as usize * 4;
            over(&mut buf[i..i + 4], color[0], color[1], color[2], (v * a255).round() as u16);
        }
    }
}

/// Stamp one glyph's alpha mask at (gx, gy). `alpha` is 0.0-1.0.
fn stamp_glyph(
    buf: &mut [u8], w: i32, h: i32,
    gx: f32, gy: f32, g: &Glyph, color: [u8; 3], alpha: f32,
) {
    let a255 = alpha.clamp(0.0, 1.0);
    for gy_ in 0..g.h as i32 {
        let py = gy as i32 + gy_;
        if py < 0 || py >= h {
            continue;
        }
        for gx_ in 0..g.w as i32 {
            let px = gx as i32 + gx_;
            if px < 0 || px >= w {
                continue;
            }
            let m = g.mask[(gy_ * g.w as i32 + gx_) as usize];
            if m == 0 {
                continue;
            }
            let i = (py * w + px) as usize * 4;
            over(&mut buf[i..i + 4], color[0], color[1], color[2], (m as f32 * a255).round() as u16);
        }
    }
}
