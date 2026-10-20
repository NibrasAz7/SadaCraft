//! Physical webcam capture via Media Foundation (M3).
//!
//! `Webcam` wraps `IMFSourceReader`: enumerates video capture devices
//! (`MFEnumDeviceSources`), negotiates RGB32 through the reader's video
//! processor where the driver allows, else reads a native NV12/YUY2 type
//! and converts. `read_frame` always returns a tightly-packed BGRX
//! buffer at `width` x `height` — the shm feed format.

#![cfg(target_os = "windows")]

use windows::core::*;
use windows::Win32::Foundation::E_UNEXPECTED;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

fn eof_err() -> Error {
    Error::from_hresult(HRESULT::from_win32(
        windows::Win32::Foundation::ERROR_HANDLE_EOF.0,
    ))
}

fn win_err(status: windows::Win32::Foundation::WIN32_ERROR) -> Error {
    Error::from_hresult(HRESULT::from_win32(status.0))
}

/// Subtypes `read_frame` can convert to BGRX.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PixelFormat {
    Rgb32,
    Nv12,
    Yuy2,
}

fn supported(subtype: &GUID) -> Option<PixelFormat> {
    if *subtype == MFVideoFormat_RGB32 {
        Some(PixelFormat::Rgb32)
    } else if *subtype == MFVideoFormat_NV12 {
        Some(PixelFormat::Nv12)
    } else if *subtype == MFVideoFormat_YUY2 {
        Some(PixelFormat::Yuy2)
    } else {
        None
    }
}

/// A live capture session. `width`/`height`/`format` describe what the
/// driver actually produces (may differ from what was requested).
pub struct Webcam {
    reader: IMFSourceReader,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    stride: i32, // signed bytes/row; negative = bottom-up
    pub name: String,
}

/// (activate, friendly name) for every video capture device present.
fn enumerate_devices() -> Result<Vec<(IMFActivate, String)>> {
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.unwrap();
        attrs.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )?;
        let mut acts: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attrs, &mut acts, &mut count)?;
        let mut out = Vec::new();
        if !acts.is_null() {
            let slice = std::slice::from_raw_parts_mut(acts, count as usize);
            for slot in slice.iter_mut() {
                if let Some(act) = slot.take() {
                    let name = allocated_string(&act, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
                        .unwrap_or_else(|| "<unnamed>".into());
                    out.push((act, name));
                }
            }
            CoTaskMemFree(Some(acts as *const _));
        }
        Ok(out)
    }
}

/// Friendly names of all video capture devices (for `cameras` listing).
pub fn list_devices() -> Result<Vec<String>> {
    unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL)? };
    let names = enumerate_devices().map(|v| v.into_iter().map(|(_, n)| n).collect());
    unsafe {
        let _ = MFShutdown();
    }
    names
}

unsafe fn allocated_string(attrs: &IMFAttributes, key: &GUID) -> Option<String> {
    let mut p = PWSTR::null();
    let mut len = 0u32;
    attrs.GetAllocatedString(key, &mut p, &mut len).ok()?;
    if p.is_null() {
        return None;
    }
    let s = PCWSTR(p.0 as *const u16).to_string().ok();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

fn frame_size(mt: &IMFMediaType) -> (u32, u32) {
    unsafe {
        let v = mt.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
        ((v >> 32) as u32, v as u32)
    }
}

fn try_set_rgb32(reader: &IMFSourceReader, w: u32, h: u32) -> Result<()> {
    unsafe {
        let mt = MFCreateMediaType()?;
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        mt.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
        reader.SetCurrentMediaType(STREAM, None, &mt)
    }
}

impl Webcam {
    /// Open a webcam and negotiate a readable format. `name_filter` is a
    /// case-insensitive substring of the friendly name; `None` picks the
    /// first device that is not the SadaCraft virtual camera itself.
    /// `(want_w, want_h)` is the preferred output size (the shm slot
    /// size); a native mode is used when RGB32 cannot be negotiated at
    /// that size and the caller rescales.
    pub fn open(name_filter: Option<&str>, want_w: u32, want_h: u32) -> Result<Self> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
            let devices = enumerate_devices()?;
            let filter = name_filter.map(|s| s.to_lowercase());
            let picked = devices.iter().find(|(_, n)| {
                match &filter {
                    Some(f) => n.to_lowercase().contains(f.as_str()),
                    // Never auto-select our own virtual camera (feedback loop).
                    None => !n.to_lowercase().contains("sadacraft"),
                }
            });
            let Some((act, name)) = picked else {
                let _ = MFShutdown();
                return Err(win_err(windows::Win32::Foundation::ERROR_FILE_NOT_FOUND));
            };
            let name = name.clone();
            let source: IMFMediaSource = act.ActivateObject()?;

            let mut rattrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut rattrs, 1)?;
            let rattrs = rattrs.unwrap();
            // Lets the reader insert converters (YUV -> RGB32 etc.).
            rattrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
            let reader = MFCreateSourceReaderFromMediaSource(&source, &rattrs)?;

            // Native media types, in driver preference order.
            let mut natives: Vec<(IMFMediaType, u32, u32, GUID)> = Vec::new();
            for i in 0..64u32 {
                match reader.GetNativeMediaType(STREAM, i) {
                    Ok(mt) => {
                        let (w, h) = frame_size(&mt);
                        let sub = mt.GetGUID(&MF_MT_SUBTYPE).unwrap_or(GUID::zeroed());
                        natives.push((mt, w, h, sub));
                    }
                    Err(_) => break,
                }
            }

            // 1) RGB32 at the requested size.
            // 2) RGB32 at each distinct native size (reader converts).
            // 3) A native type we can convert ourselves.
            let mut negotiated = try_set_rgb32(&reader, want_w, want_h);
            if negotiated.is_err() {
                let mut seen = Vec::new();
                for (_, w, h, _) in &natives {
                    if seen.contains(&(*w, *h)) {
                        continue;
                    }
                    seen.push((*w, *h));
                    negotiated = try_set_rgb32(&reader, *w, *h);
                    if negotiated.is_ok() {
                        break;
                    }
                }
            }
            if negotiated.is_err() {
                for (mt, _, _, sub) in &natives {
                    if supported(sub).is_some()
                        && reader.SetCurrentMediaType(STREAM, None, mt).is_ok()
                    {
                        negotiated = Ok(());
                        break;
                    }
                }
            }
            negotiated?;

            // What we actually got.
            let cur = reader.GetCurrentMediaType(STREAM)?;
            let (w, h) = frame_size(&cur);
            let sub = cur.GetGUID(&MF_MT_SUBTYPE).unwrap_or(GUID::zeroed());
            let Some(format) = supported(&sub) else {
                let _ = MFShutdown();
                return Err(Error::from_hresult(E_UNEXPECTED));
            };
            let stride = cur
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|v| v as i32)
                .unwrap_or_else(|_| {
                    (w * match format {
                        PixelFormat::Rgb32 => 4,
                        PixelFormat::Yuy2 => 2,
                        PixelFormat::Nv12 => 1,
                    }) as i32
                });
            Ok(Webcam {
                reader,
                width: w,
                height: h,
                format,
                stride,
                name,
            })
        }
    }

    /// Block until the next frame; returns tight BGRX at `width`x`height`.
    /// `Err(EOF)` on end of stream, other `Err` on device faults.
    pub fn read_frame(&mut self) -> Result<Vec<u8>> {
        unsafe {
            loop {
                let mut flags = 0u32;
                let mut sample: Option<IMFSample> = None;
                self.reader.ReadSample(
                    STREAM,
                    0,
                    None,
                    Some(&mut flags),
                    None,
                    Some(&mut sample),
                )?;
                if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                    return Err(eof_err());
                }
                let Some(sample) = sample else { continue };
                let buf = sample.ConvertToContiguousBuffer()?;
                let mut p = std::ptr::null_mut();
                let mut cur_len = 0u32;
                buf.Lock(&mut p, None, Some(&mut cur_len))?;
                let src = std::slice::from_raw_parts(p, cur_len as usize);
                let out = self.convert(src);
                let _ = buf.Unlock();
                return Ok(out);
            }
        }
    }

    fn convert(&self, src: &[u8]) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![0u8; w * h * 4];
        match self.format {
            PixelFormat::Rgb32 => copy_rgb32(src, self.stride, w, h, &mut out),
            PixelFormat::Nv12 => nv12_to_bgrx(src, self.stride, w, h, &mut out),
            PixelFormat::Yuy2 => yuy2_to_bgrx(src, self.stride, w, h, &mut out),
        }
        out
    }
}

impl Drop for Webcam {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}

/// RGB32 (already BGRX) copy honoring stride and bottom-up sign.
fn copy_rgb32(src: &[u8], stride: i32, w: usize, h: usize, out: &mut [u8]) {
    let abs = stride.unsigned_abs() as usize;
    let row = (w * 4).min(abs);
    if abs * h > src.len() {
        return;
    }
    for y in 0..h {
        let sy = if stride < 0 { h - 1 - y } else { y };
        out[y * w * 4..y * w * 4 + row].copy_from_slice(&src[sy * abs..sy * abs + row]);
    }
}

#[inline]
fn clamp8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// BT.601 limited-range YUV -> BGR, shared by NV12 and YUY2 paths.
#[inline]
fn yuv_to_bgr(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let c = y as i32 - 16;
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    (
        clamp8((298 * c + 516 * d + 128) >> 8),
        clamp8((298 * c - 100 * d - 208 * e + 128) >> 8),
        clamp8((298 * c + 409 * e + 128) >> 8),
    )
}

/// NV12: `h` rows of Y at `stride`, then `h/2` rows of interleaved UV.
fn nv12_to_bgrx(src: &[u8], stride: i32, w: usize, h: usize, out: &mut [u8]) {
    let s = stride.unsigned_abs() as usize;
    if s * h + s * h / 2 > src.len() {
        return;
    }
    let uv_base = s * h;
    for y in 0..h {
        for x in 0..w {
            let luma = src[y * s + x];
            let uv = uv_base + (y / 2) * s + (x & !1);
            let (b, g, r) = yuv_to_bgr(luma, src[uv], src[uv + 1]);
            let d = (y * w + x) * 4;
            out[d] = b;
            out[d + 1] = g;
            out[d + 2] = r;
            out[d + 3] = 0xFF;
        }
    }
}

/// YUY2: 4 bytes per 2 pixels (Y0 U Y1 V).
fn yuy2_to_bgrx(src: &[u8], stride: i32, w: usize, h: usize, out: &mut [u8]) {
    let s = stride.unsigned_abs() as usize;
    if s * h > src.len() {
        return;
    }
    for y in 0..h {
        for x in 0..w {
            let o = y * s + x * 2;
            let luma = src[o];
            let (u, v) = if x & 1 == 0 {
                (src[o + 1], src[o + 3])
            } else {
                (src[o - 1], src[o + 1])
            };
            let (b, g, r) = yuv_to_bgr(luma, u, v);
            let d = (y * w + x) * 4;
            out[d] = b;
            out[d + 1] = g;
            out[d + 2] = r;
            out[d + 3] = 0xFF;
        }
    }
}
