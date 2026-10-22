//! Person segmentation -> background replacement for `sadacam-cli feed` (M3).
//!
//! MediaPipe selfie_segmenter exported to ONNX (256x256 MobileNetV3,
//! Apache-2.0) run on the producer side — the COM media source inside Frame
//! Server's host never touches ONNX Runtime.
//!
//! Model: https://huggingface.co/onnx-community/mediapipe_selfie_segmentation
//! (`onnx/model.onnx`, fp32, ~500 kB). On first use it is downloaded into the
//! first writable of `%LOCALAPPDATA%\SadaCraft\models\`, the exe directory,
//! or `%PROGRAMDATA%\SadaCraft\models\`; `--model <path>` overrides entirely.
//!
//! The onnxruntime DLL is resolved in this order (same pattern src-tauri
//! uses): `ORT_DYLIB_PATH`, `onnxruntime.dll` next to the exe,
//! `<exe>\onnxruntime\onnxruntime.dll`, then the dev-layout fallback
//! `app\frontend\src-tauri\binaries\onnxruntime\onnxruntime.dll` relative to
//! this crate's manifest dir. `ort` is built with `load-dynamic`, so nothing
//! is fetched or linked at build time — the DLL is dlopen'd on first use.
//!
//! Failure anywhere (no DLL, no model, inference error) is non-fatal: the
//! caller logs once and keeps feeding raw frames.

#![cfg(target_os = "windows")]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows::core::*;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Networking::WinInet::*;

const MODEL_URL: &str =
    "https://huggingface.co/onnx-community/mediapipe_selfie_segmentation/resolve/main/onnx/model.onnx";
const MODEL_FILE: &str = "selfie_segmenter.onnx";

fn err(msg: impl std::fmt::Display) -> Error {
    Error::new(E_FAIL, &msg.to_string())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// --- onnxruntime.dll resolution + one-time init ----------------------------

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// Candidate onnxruntime.dll locations, in priority order.
fn ort_dll_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("ORT_DYLIB_PATH") {
        v.push(PathBuf::from(p));
    }
    if let Some(dir) = exe_dir() {
        v.push(dir.join("onnxruntime.dll"));
        v.push(dir.join("onnxruntime").join("onnxruntime.dll"));
    }
    // Dev-layout fallback: the bundled copy shipped for the Tauri app
    // (app/frontend/src-tauri/binaries/onnxruntime/onnxruntime.dll).
    v.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("frontend")
            .join("src-tauri")
            .join("binaries")
            .join("onnxruntime")
            .join("onnxruntime.dll"),
    );
    v
}

/// `ort::init_from`/`commit` can surface loader failures as panics (mirrors
/// src-tauri's `catch_onnx_runtime_init`): fold them into an error string.
fn catch_init<F>(f: F) -> std::result::Result<(), String>
where
    F: FnOnce() -> std::result::Result<(), String>,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            Err(format!("ONNX Runtime init panicked: {msg}"))
        }
    }
}

static ORT_INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();

/// Initialize ONNX Runtime once from a resolvable DLL path. `Err(String)`
/// names every candidate that was tried.
fn ensure_ort() -> std::result::Result<(), String> {
    ORT_INIT
        .get_or_init(|| {
            let mut tried = Vec::new();
            for dll in ort_dll_candidates() {
                if !dll.is_file() {
                    continue;
                }
                let path = dll.to_string_lossy().into_owned();
                let r = catch_init(|| {
                    ort::init_from(path.clone())
                        .with_telemetry(false)
                        .commit()
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
                match r {
                    Ok(()) => return Ok(()),
                    Err(e) => tried.push(format!("{} ({e})", dll.display())),
                }
            }
            if tried.is_empty() {
                Err("onnxruntime.dll not found (set ORT_DYLIB_PATH or place it \
                     next to sadacam-cli.exe)"
                    .to_string())
            } else {
                Err(format!("onnxruntime.dll failed to load: {}", tried.join("; ")))
            }
        })
        .clone()
}

// --- model file resolution + download --------------------------------------

/// Directories that may hold the cached model, in search order; the first
/// writable one is also the download target.
fn model_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("LOCALAPPDATA") {
        v.push(Path::new(&p).join("SadaCraft").join("models"));
    }
    if let Some(dir) = exe_dir() {
        v.push(dir.join("models"));
    }
    if let Some(p) = std::env::var_os("PROGRAMDATA") {
        v.push(Path::new(&p).join("SadaCraft").join("models"));
    }
    v
}

/// Resolve the model file: explicit `--model` path wins and must exist;
/// otherwise the first cached copy found, else a download into the first
/// writable model directory.
pub fn resolve_model(explicit: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        let path = PathBuf::from(p);
        if !path.is_file() {
            return Err(err(format!("model not found: {}", path.display())));
        }
        return Ok(path);
    }
    for dir in model_dirs() {
        let p = dir.join(MODEL_FILE);
        if p.is_file() {
            return Ok(p);
        }
    }
    // Not cached — pick the first directory we can create.
    let mut last = String::from("no candidate directories");
    for dir in model_dirs() {
        match std::fs::create_dir_all(&dir) {
            Ok(()) => {
                let dst = dir.join(MODEL_FILE);
                download(MODEL_URL, &dst)?;
                return Ok(dst);
            }
            Err(e) => last = format!("{}: {e}", dir.display()),
        }
    }
    Err(err(format!("no writable model directory ({last})")))
}

/// Minimal HTTPS GET via WinINet (no extra crates). Writes `dst` atomically
/// through a `.part` temp file.
fn download(url: &str, dst: &Path) -> Result<()> {
    unsafe {
        let agent = wide("sadacam-cli/0.1");
        let inet = InternetOpenW(
            PCWSTR(agent.as_ptr()),
            INTERNET_OPEN_TYPE_PRECONFIG.0,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        );
        if inet.is_null() {
            return Err(Error::from_thread());
        }
        let url_w = wide(url);
        let conn = InternetOpenUrlW(
            inet,
            PCWSTR(url_w.as_ptr()),
            None,
            INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_CACHE_WRITE | INTERNET_FLAG_SECURE,
            None,
        );
        if conn.is_null() {
            let e = Error::from_thread();
            let _ = InternetCloseHandle(inet);
            return Err(e);
        }
        let tmp = dst.with_extension("onnx.part");
        let mut out = match std::fs::File::create(&tmp) {
            Ok(f) => f,
            Err(e) => {
                let _ = InternetCloseHandle(conn);
                let _ = InternetCloseHandle(inet);
                return Err(err(e));
            }
        };
        let mut result = Ok(());
        let mut buf = [0u8; 64 * 1024];
        loop {
            let mut got = 0u32;
            if let Err(e) = InternetReadFile(
                conn,
                buf.as_mut_ptr() as *mut _,
                buf.len() as u32,
                &mut got,
            ) {
                result = Err(e);
                break;
            }
            if got == 0 {
                break;
            }
            use std::io::Write;
            if let Err(e) = out.write_all(&buf[..got as usize]) {
                result = Err(err(e));
                break;
            }
        }
        drop(out);
        let _ = InternetCloseHandle(conn);
        let _ = InternetCloseHandle(inet);
        match result {
            Ok(()) => {
                std::fs::rename(&tmp, dst)?;
                Ok(())
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }
}

// --- background mode --------------------------------------------------------

/// What replaces the non-person region.
pub enum Background {
    /// Separable box blur; `radius` is pixels each side (5 -> ~11 px kernel).
    Blur { radius: usize },
    /// Solid color, B/G/R order.
    Color(u8, u8, u8),
}

impl Background {
    /// Parse `--background` values: `blur`, `blur:<radius>`,
    /// `color:#RRGGBB`, or a bare `#RRGGBB`.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s == "blur" {
            return Ok(Background::Blur { radius: 5 });
        }
        if let Some(r) = s.strip_prefix("blur:") {
            let radius: usize = r.parse().map_err(|_| err("bad blur radius"))?;
            return Ok(Background::Blur {
                radius: radius.clamp(1, 50),
            });
        }
        let hex = s
            .strip_prefix("color:")
            .unwrap_or(s)
            .strip_prefix('#')
            .unwrap_or(s);
        if hex.len() == 6 {
            if let Ok(v) = u32::from_str_radix(hex, 16) {
                return Ok(Background::Color(
                    (v & 0xFF) as u8,
                    ((v >> 8) & 0xFF) as u8,
                    ((v >> 16) & 0xFF) as u8,
                ));
            }
        }
        Err(err(format!(
            "bad --background '{s}' (use blur | blur:<px> | color:#RRGGBB)"
        )))
    }
}

// --- segmentation session ---------------------------------------------------

/// Loaded selfie-segmentation session. `infer_mask` returns a float person
/// mask at model resolution; `apply` composites the background in place.
pub struct Segmenter {
    session: ort::session::Session,
    input_name: String,
    output_name: String,
    nchw: bool,
    mw: usize,
    mh: usize,
}

impl Segmenter {
    /// Load `model_path` through the dynamically-bound ONNX Runtime.
    pub fn open(model_path: &Path) -> Result<Self> {
        ensure_ort().map_err(err)?;
        use ort::execution_providers::CPUExecutionProvider;
        use ort::session::builder::GraphOptimizationLevel;
        use ort::session::Session;
        let providers = vec![CPUExecutionProvider::default().build()];
        let session = Session::builder()
            .map_err(err)?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(err)?
            .with_execution_providers(providers)
            .map_err(err)?
            .with_intra_threads(2)
            .map_err(err)?
            .commit_from_file(model_path)
            .map_err(err)?;

        let (input_name, nchw, mh, mw) = {
            let input = session
                .inputs
                .first()
                .ok_or_else(|| err("segmentation model has no inputs"))?;
            // Input layout from the declared shape: [1,3,H,W] or [1,H,W,3];
            // dynamic dims fall back to the model's published 256.
            let (nchw, mh, mw) = match &input.input_type {
                ort::value::ValueType::Tensor { shape, .. } => {
                    let d: Vec<i64> = shape.iter().copied().collect();
                    if d.len() == 4 && d[3] == 3 {
                        (false, pos(d[1]), pos(d[2]))
                    } else {
                        (
                            true,
                            pos(d.get(2).copied().unwrap_or(-1)),
                            pos(d.get(3).copied().unwrap_or(-1)),
                        )
                    }
                }
                _ => (true, 256, 256),
            };
            (input.name.clone(), nchw, mh, mw)
        };
        let output_name = session
            .outputs
            .first()
            .ok_or_else(|| err("segmentation model has no outputs"))?
            .name
            .clone();
        Ok(Segmenter {
            session,
            input_name,
            output_name,
            nchw,
            mw,
            mh,
        })
    }

    /// BGRX frame -> person-probability mask at model res (row-major mh*mw).
    pub fn infer_mask(&mut self, frame: &[u8], w: usize, h: usize) -> Result<(Vec<f32>, usize, usize)> {
        let (mw, mh) = (self.mw, self.mh);
        // Bilinear downsample + rescale to [0,1] RGB (the model applies no
        // further normalization; rescale_factor = 1/255).
        let mut data = vec![0f32; mw * mh * 3];
        for y in 0..mh {
            let fy = ((y as f32 + 0.5) * h as f32 / mh as f32 - 0.5)
                .clamp(0.0, (h - 1) as f32);
            let y0 = fy.floor() as usize;
            let y1 = (y0 + 1).min(h - 1);
            let ty = fy.fract();
            for x in 0..mw {
                let fx = ((x as f32 + 0.5) * w as f32 / mw as f32 - 0.5)
                    .clamp(0.0, (w - 1) as f32);
                let x0 = fx.floor() as usize;
                let x1 = (x0 + 1).min(w - 1);
                let tx = fx.fract();
                let mut px = [0f32; 3]; // B,G,R
                for (sx, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                    for (sy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                        let s = (sy * w + sx) * 4;
                        let wgt = wx * wy;
                        px[0] += frame[s] as f32 * wgt;
                        px[1] += frame[s + 1] as f32 * wgt;
                        px[2] += frame[s + 2] as f32 * wgt;
                    }
                }
                // Model expects RGB in [0,1].
                if self.nchw {
                    data[0 * mh * mw + y * mw + x] = px[2] / 255.0;
                    data[1 * mh * mw + y * mw + x] = px[1] / 255.0;
                    data[2 * mh * mw + y * mw + x] = px[0] / 255.0;
                } else {
                    let o = (y * mw + x) * 3;
                    data[o] = px[2] / 255.0;
                    data[o + 1] = px[1] / 255.0;
                    data[o + 2] = px[0] / 255.0;
                }
            }
        }

        let shape: Vec<usize> = if self.nchw {
            vec![1, 3, mh, mw]
        } else {
            vec![1, mh, mw, 3]
        };
        let arr = ndarray::Array::from_shape_vec(ndarray::IxDyn(&shape), data)
            .map_err(err)?;
        let run_inputs = ort::inputs![self.input_name.clone() =>
            ort::value::TensorRef::from_array_view(arr.view()).map_err(err)?];
        let outputs = self.session.run(run_inputs).map_err(err)?;
        let out = outputs
            .get(self.output_name.as_str())
            .ok_or_else(|| err(format!("model output '{}' missing", self.output_name)))?
            .try_extract_array::<f32>()
            .map_err(err)?;
        let mask: Vec<f32> = out.iter().copied().collect();
        let (omh, omw) = mask_hw(out.shape(), mask.len())
            .ok_or_else(|| err("unhandled mask shape"))?;
        Ok((mask, omw, omh))
    }

    /// Run inference and composite `bg` behind the person, in place.
    pub fn apply(&mut self, frame: &mut [u8], w: usize, h: usize, bg: &Background) -> Result<()> {
        let (mask, mw, mh) = self.infer_mask(frame, w, h)?;
        composite(frame, w, h, &mask, mw, mh, bg);
        Ok(())
    }
}

/// Composite `bg` behind the person using a precomputed float person mask
/// (row-major `mh*mw`, bilinear-sampled up to frame res). Split out from
/// `Segmenter::apply` so the effects pipeline can reuse a mask it already
/// cropped through auto-frame instead of re-running inference.
pub fn composite(frame: &mut [u8], w: usize, h: usize, mask: &[f32], mw: usize, mh: usize, bg: &Background) {
    let blurred = match bg {
        Background::Blur { radius } => Some(box_blur(frame, w, h, *radius)),
        Background::Color(..) => None,
    };
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
            let d = (y * w + x) * 4;
            let ia = 1.0 - m;
            for c in 0..3 {
                let bgv = match bg {
                    Background::Blur { .. } => blurred.as_ref().unwrap()[d + c] as f32,
                    Background::Color(b, g, r) => [*b, *g, *r][c] as f32,
                };
                frame[d + c] = (frame[d + c] as f32 * m + bgv * ia) as u8;
            }
        }
    }
}

fn pos(dim: i64) -> usize {
    if dim > 0 {
        dim as usize
    } else {
        256
    }
}

/// (h, w) of the mask tensor: the last two non-1 dims when they multiply to
/// the element count ([1,1,H,W], [1,H,W,1]), else a square fallback.
fn mask_hw(shape: &[usize], len: usize) -> Option<(usize, usize)> {
    let big: Vec<usize> = shape.iter().copied().filter(|&d| d > 1).collect();
    if big.len() >= 2 {
        let (a, b) = (big[big.len() - 2], big[big.len() - 1]);
        if a * b == len {
            return Some((a, b));
        }
    }
    let s = (len as f64).sqrt() as usize;
    if s * s == len {
        Some((s, s))
    } else {
        None
    }
}

/// Separable box blur on B/G/R (alpha kept), edge-clamped sliding window.
fn box_blur(src: &[u8], w: usize, h: usize, radius: usize) -> Vec<u8> {
    let r = radius as i32;
    let mut tmp = vec![0u8; src.len()];
    let mut dst = vec![0u8; src.len()];
    // Horizontal pass.
    for y in 0..h {
        let base = y * w * 4;
        let (mut sb, mut sg, mut sr, mut n) = (0u32, 0u32, 0u32, 0u32);
        for x in 0..=radius.min(w - 1) {
            let s = base + x * 4;
            sb += src[s] as u32;
            sg += src[s + 1] as u32;
            sr += src[s + 2] as u32;
            n += 1;
        }
        for x in 0..w {
            let d = base + x * 4;
            tmp[d] = (sb / n) as u8;
            tmp[d + 1] = (sg / n) as u8;
            tmp[d + 2] = (sr / n) as u8;
            tmp[d + 3] = 0xFF;
            let xa = x + radius + 1;
            if xa < w {
                let s = base + xa * 4;
                sb += src[s] as u32;
                sg += src[s + 1] as u32;
                sr += src[s + 2] as u32;
                n += 1;
            }
            let xr = x as i32 - r;
            if xr >= 0 {
                let s = base + xr as usize * 4;
                sb -= src[s] as u32;
                sg -= src[s + 1] as u32;
                sr -= src[s + 2] as u32;
                n -= 1;
            }
        }
    }
    // Vertical pass on tmp -> dst.
    for x in 0..w {
        let (mut sb, mut sg, mut sr, mut n) = (0u32, 0u32, 0u32, 0u32);
        for y in 0..=radius.min(h - 1) {
            let s = (y * w + x) * 4;
            sb += tmp[s] as u32;
            sg += tmp[s + 1] as u32;
            sr += tmp[s + 2] as u32;
            n += 1;
        }
        for y in 0..h {
            let d = (y * w + x) * 4;
            dst[d] = (sb / n) as u8;
            dst[d + 1] = (sg / n) as u8;
            dst[d + 2] = (sr / n) as u8;
            dst[d + 3] = 0xFF;
            let ya = y + radius + 1;
            if ya < h {
                let s = (ya * w + x) * 4;
                sb += tmp[s] as u32;
                sg += tmp[s + 1] as u32;
                sr += tmp[s + 2] as u32;
                n += 1;
            }
            let yr = y as i32 - r;
            if yr >= 0 {
                let s = (yr as usize * w + x) * 4;
                sb -= tmp[s] as u32;
                sg -= tmp[s + 1] as u32;
                sr -= tmp[s + 2] as u32;
                n -= 1;
            }
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_parse() {
        assert!(matches!(
            Background::parse("blur").unwrap(),
            Background::Blur { radius: 5 }
        ));
        assert!(matches!(
            Background::parse("color:#FF8800").unwrap(),
            Background::Color(0x00, 0x88, 0xFF)
        ));
        assert!(Background::parse("bogus").is_err());
    }

    #[test]
    fn mask_hw_shapes() {
        assert_eq!(mask_hw(&[1, 1, 256, 256], 65536), Some((256, 256)));
        assert_eq!(mask_hw(&[1, 144, 256, 1], 144 * 256), Some((144, 256)));
        assert_eq!(mask_hw(&[256, 256], 65536), Some((256, 256)));
    }

    /// End-to-end: model resolution (download on first run) -> onnxruntime.dll
    /// -> session -> mask + composite on a synthetic frame. Skips quietly when
    /// the machine has no network/model or no onnxruntime.dll (mirrors
    /// src-tauri's approach of gating ort-dependent tests on resource
    /// presence).
    #[test]
    fn segmenter_runs_when_model_and_runtime_present() {
        let Ok(model) = resolve_model(None) else {
            return;
        };
        if ort_dll_candidates().iter().all(|p| !p.is_file()) {
            return;
        }
        let mut seg = Segmenter::open(&model).expect("segmenter failed to open");
        let (w, h) = (64usize, 64usize);
        // Gray background with a bright center blob.
        let mut frame = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let d = (y * w + x) * 4;
                let v = if (16..48).contains(&x) && (8..56).contains(&y) {
                    220u8
                } else {
                    90
                };
                frame[d] = v;
                frame[d + 1] = v;
                frame[d + 2] = v;
                frame[d + 3] = 0xFF;
            }
        }
        let (mask, mw, mh) = seg.infer_mask(&frame, w, h).expect("inference failed");
        assert_eq!(mask.len(), mw * mh);
        assert!(mask.iter().all(|m| m.is_finite()));
        let mut composited = frame.clone();
        seg.apply(&mut composited, w, h, &Background::Color(255, 0, 0))
            .expect("composite failed");
        let mut blurred = frame.clone();
        seg.apply(&mut blurred, w, h, &Background::Blur { radius: 5 })
            .expect("blur composite failed");
    }

    #[test]
    fn blur_is_local_average() {
        // 4x4, left half black, right half white — blur blends the seam.
        let (w, h) = (4usize, 4usize);
        let mut src = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let d = (y * w + x) * 4;
                let v = if x >= 2 { 255u8 } else { 0 };
                src[d] = v;
                src[d + 1] = v;
                src[d + 2] = v;
                src[d + 3] = 0xFF;
            }
        }
        let out = box_blur(&src, w, h, 1);
        // 3x3 window at pixel (1,1): 3 of 9 samples white -> ~85.
        let left = out[(1 * w + 1) * 4];
        // 3x3 window at pixel (2,1): 6 of 9 samples white -> ~170.
        let right = out[(1 * w + 2) * 4];
        assert!((left as i32 - 85).abs() < 20, "v={left}");
        assert!((right as i32 - 170).abs() < 20, "v={right}");
    }
}
