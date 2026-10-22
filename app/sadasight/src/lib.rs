//! SadaSight — SadaCraft screen spotlight / pointer-effects overlay.
//!
//! Standalone crate (sibling of `sadacam`): a click-through, always-on-top
//! layered window covering the virtual screen, rendering cursor-following
//! effects in software (premultiplied BGRA via `UpdateLayeredWindow`).
//!
//! `overlay::run` blocks on a Win32 message loop; `effects::Engine` is pure
//! Rust so the effect renderer can later be driven by the Tauri app directly.

pub mod config;
pub mod effects;
pub mod overlay;

pub use config::Config;
