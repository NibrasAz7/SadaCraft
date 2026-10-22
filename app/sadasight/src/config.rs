//! Effect configuration + JSON persistence.
//!
//! Saved to `%APPDATA%\com.sadacraft.app\sadasight.json` (override with
//! `--config <file>`). Colors are `[r, g, b]`; radii/thickness in pixels;
//! `opacity` and `dim` are 0.0-1.0.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const EFFECT_NAMES: [&str; 6] = ["ring", "ripple", "trail", "spotlight", "pen", "keys"];

/// Bit index for an effect name (`all` is handled by the caller).
pub fn effect_index(name: &str) -> Option<usize> {
    EFFECT_NAMES.iter().position(|n| *n == name)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Master switch: overlay window shown at all.
    pub visible: bool,
    pub ring: RingConfig,
    pub ripple: RippleConfig,
    pub trail: TrailConfig,
    pub spotlight: SpotlightConfig,
    pub pen: PenConfig,
    pub keys: KeysConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RingConfig {
    pub enabled: bool,
    pub color: [u8; 3],
    pub radius: f32,
    pub thickness: f32,
    pub opacity: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RippleConfig {
    pub enabled: bool,
    pub color: [u8; 3],
    pub duration_ms: u64,
    pub max_radius: f32,
    pub thickness: f32,
    pub opacity: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TrailConfig {
    pub enabled: bool,
    pub color: [u8; 3],
    pub lifetime_ms: u64,
    pub radius: f32,
    pub opacity: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SpotlightConfig {
    pub enabled: bool,
    /// Clear radius around the cursor.
    pub radius: f32,
    /// Darkness of the dimmed area, 0.0-1.0.
    pub dim: f32,
    /// Soft transition width between clear and dimmed.
    pub feather: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PenConfig {
    /// Pen mode (click-through disabled, freehand strokes).
    pub enabled: bool,
    /// Stroke palette; keys 1-4 select while pen mode is active.
    pub colors: Vec<[u8; 3]>,
    /// Stroke width in pixels.
    pub width: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct KeysConfig {
    /// Keystroke visualization: fading chips showing recent key presses.
    pub enabled: bool,
    /// How long a chip stays on screen.
    pub lifetime_ms: u64,
    /// Text color.
    pub color: [u8; 3],
    /// Chip background color.
    pub bg: [u8; 3],
    /// Max opacity of chip background/text, 0.0-1.0.
    pub opacity: f32,
    /// Distance above the primary monitor's bottom edge.
    pub margin_bottom: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            visible: true,
            ring: RingConfig::default(),
            ripple: RippleConfig::default(),
            trail: TrailConfig::default(),
            spotlight: SpotlightConfig::default(),
            pen: PenConfig::default(),
            keys: KeysConfig::default(),
        }
    }
}

impl Default for RingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            color: [255, 200, 40],
            radius: 24.0,
            thickness: 3.5,
            opacity: 0.9,
        }
    }
}

impl Default for RippleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            color: [255, 90, 60],
            duration_ms: 600,
            max_radius: 90.0,
            thickness: 4.0,
            opacity: 0.9,
        }
    }
}

impl Default for TrailConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            color: [255, 60, 40],
            lifetime_ms: 1400,
            radius: 9.0,
            opacity: 0.85,
        }
    }
}

impl Default for SpotlightConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            radius: 200.0,
            dim: 0.62,
            feather: 60.0,
        }
    }
}

impl Default for PenConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            colors: vec![
                [255, 60, 50],
                [60, 180, 255],
                [90, 220, 120],
                [255, 210, 40],
            ],
            width: 5.0,
        }
    }
}

impl Default for KeysConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            lifetime_ms: 2200,
            color: [235, 235, 240],
            bg: [24, 24, 30],
            opacity: 0.92,
            margin_bottom: 72.0,
        }
    }
}

impl Config {
    /// Bit mask of enabled effects (bit order = `EFFECT_NAMES`).
    pub fn mask(&self) -> u8 {
        (0..6).fold(0u8, |m, i| m | ((self.effect_enabled(i) as u8) << i))
    }

    pub fn effect_enabled(&self, i: usize) -> bool {
        match i {
            0 => self.ring.enabled,
            1 => self.ripple.enabled,
            2 => self.trail.enabled,
            3 => self.spotlight.enabled,
            4 => self.pen.enabled,
            5 => self.keys.enabled,
            _ => false,
        }
    }

    pub fn set_effect(&mut self, i: usize, on: bool) {
        match i {
            0 => self.ring.enabled = on,
            1 => self.ripple.enabled = on,
            2 => self.trail.enabled = on,
            3 => self.spotlight.enabled = on,
            4 => self.pen.enabled = on,
            5 => self.keys.enabled = on,
            _ => {}
        }
    }

    /// Apply a toggle/force-on/force-off mask triple (CLI semantics).
    pub fn apply_masks(&mut self, toggle: u8, on: u8, off: u8) {
        for i in 0..6 {
            let b = 1u8 << i;
            let cur = self.effect_enabled(i);
            let new = if toggle & b != 0 {
                !cur
            } else if on & b != 0 {
                true
            } else if off & b != 0 {
                false
            } else {
                cur
            };
            self.set_effect(i, new);
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).unwrap_or_default();
        std::fs::write(path, json)
    }
}

/// `%APPDATA%\com.sadacraft.app\sadasight.json`
pub fn default_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base)
        .join("com.sadacraft.app")
        .join("sadasight.json")
}

pub fn load(path: &Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("[sadasight] ignoring invalid config {}: {e}", path.display());
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}
