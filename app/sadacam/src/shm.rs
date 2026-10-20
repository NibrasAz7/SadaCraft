//! Named shared-memory frame feed (M2).
//!
//! A single-producer frame slot in the `Global\` namespace. Ownership is
//! inverted relative to a naive design: **the consumer creates the
//! mapping** — Frame Server's host runs as LocalService and may create
//! `Global\` objects (interactive users cannot on this machine). The
//! producer (SadaCraft app, or `sadacam-cli feed`) opens it read-write
//! via a permissive DACL. Semantics are demand-gated: a feed attaches
//! to a live camera, not the other way around.
//!
//! Layout: 64-byte header + one BGRX frame. The writer bumps `seq`
//! (odd while writing — seqlock); the reader copies the payload and
//! records `read_seq` so the producer can see consumption.

#![cfg(target_os = "windows")]

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Security::Authorization::*;
use windows::Win32::Security::*;
use windows::Win32::System::Memory::*;

const MAPPING_NAME: &str = r"Global\SadaCamFrame";
const MAGIC: u32 = 0x5344_4346; // "SDCF"
const VERSION: u32 = 1;
const HEADER_BYTES: usize = 64;

const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_SEQ: usize = 8; // seqlock: odd while a write is in flight
const OFF_WIDTH: usize = 12;
const OFF_HEIGHT: usize = 16;
const OFF_STRIDE: usize = 20;
const OFF_FORMAT: usize = 24; // 0 = BGRX (RGB32 media type)
const OFF_PAYLOAD_LEN: usize = 28;
const OFF_READ_SEQ: usize = 32; // seq of last frame consumed by FS host
const OFF_STATUS: usize = 36; // consumer-side stage/error marker (debug)
const OFF_INSTANCE: usize = 40; // fresh token per consumer open (recycle marker)

pub const FORMAT_BGRX: u32 = 0;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn read_u32(base: *const u8, off: usize) -> u32 {
    unsafe { std::ptr::read_volatile(base.add(off) as *const u32) }
}
fn write_u32(base: *mut u8, off: usize, v: u32) {
    unsafe { std::ptr::write_volatile(base.add(off) as *mut u32, v) }
}

/// Consumer-side DACL: `CREATOR OWNER`/`SYSTEM` full, `Everyone` read+write
/// so any producer process (interactive user) can open and feed frames.
fn permissive_sd() -> Result<(SECURITY_ATTRIBUTES, *mut std::ffi::c_void)> {
    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:P(A;;GA;;;CO)(A;;GA;;;SY)(A;;GRGW;;;WD)"),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )?;
        Ok((
            SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            },
            sd.0,
        ))
    }
}

fn last_error() -> Error {
    unsafe { Error::from_hresult(HRESULT::from_win32(GetLastError().0)) }
}

/// Fresh per-open token stamped into OFF_INSTANCE by the consumer.
/// A producer compares it against the token it first saw to detect
/// consumer recycling (see `FrameWriter::maybe_reconnect`).
fn new_instance_token() -> u32 {
    // Counter guarantees distinct tokens even within one timer tick.
    static SALT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (std::process::id() << 12)
        ^ nanos
        ^ SALT
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B1)
        ^ 0x5ADA
}

/// Consumer side (media source in the FS host): creates the slot the
/// first time Frame Server pulls, then seqlock-reads the newest frame.
pub struct FrameReader {
    _mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    payload_len: usize,
}

impl FrameReader {
    fn base(&self) -> *mut u8 {
        self.view.Value as *mut u8
    }

    /// Create (or attach to) the slot for `width`x`height` BGRX frames.
    /// Runs inside the FS host process as LocalService — `Global\`
    /// creation needs a service-context token on this machine.
    pub fn open(width: u32, height: u32) -> Result<Self> {
        Self::open_named(MAPPING_NAME, width, height)
    }

    /// `open` against an explicit name — lets tests stay out of the
    /// `Global\` namespace (session-private names need no service token).
    fn open_named(name: &str, width: u32, height: u32) -> Result<Self> {
        let payload_len = (width * height * 4) as usize;
        let total = HEADER_BYTES + payload_len;
        unsafe {
            let (sa, sd) = match permissive_sd() {
                Ok(v) => v,
                Err(_) => return Err(E_UNEXPECTED.into()),
            };
            let mapping = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                Some(&sa),
                PAGE_READWRITE,
                (total >> 32) as u32,
                total as u32,
                PCWSTR(wide(name).as_ptr()),
            );
            LocalFree(Some(HLOCAL(sd)));
            let mut mapping = match mapping {
                Ok(h) => h,
                // Object exists but was created by a previous FS host —
                // our DACL only grants Everyone GR|GW, so fall back to a
                // read-write open that fits those rights.
                Err(_) => OpenFileMappingW(
                    FILE_MAP_READ.0 | FILE_MAP_WRITE.0,
                    false,
                    PCWSTR(wide(name).as_ptr()),
                )?,
            };
            let rw = FILE_MAP(FILE_MAP_READ.0 | FILE_MAP_WRITE.0);
            let mut view = MapViewOfFile(mapping, rw, 0, 0, total);
            if view.Value.is_null() {
                // Handle may carry more rights than needed; retry with the
                // least the DACL guarantees.
                let _ = CloseHandle(mapping);
                mapping = OpenFileMappingW(rw.0, false, PCWSTR(wide(name).as_ptr()))?;
                view = MapViewOfFile(mapping, rw, 0, 0, total);
            }
            if view.Value.is_null() {
                let e = last_error();
                let _ = CloseHandle(mapping);
                return Err(e);
            }
            let base = view.Value as *mut u8;
            // Initialize the header only when we created the object — a
            // prior producer may already be holding the same mapping.
            if read_u32(base, OFF_MAGIC) != MAGIC {
                write_u32(base, OFF_MAGIC, MAGIC);
                write_u32(base, OFF_VERSION, VERSION);
                write_u32(base, OFF_SEQ, 0);
                write_u32(base, OFF_READ_SEQ, 0);
            }
            // Fresh token on EVERY open: this marks a new consumer
            // generation even when we re-attach to a surviving object, so
            // a producer watching OFF_INSTANCE sees the recycle.
            write_u32(base, OFF_INSTANCE, new_instance_token());
            write_u32(base, OFF_STATUS, 1); // stage: mapped
            write_u32(base, OFF_WIDTH, width);
            write_u32(base, OFF_HEIGHT, height);
            write_u32(base, OFF_STRIDE, width * 4);
            write_u32(base, OFF_FORMAT, FORMAT_BGRX);
            write_u32(base, OFF_PAYLOAD_LEN, payload_len as u32);
            Ok(FrameReader {
                _mapping: mapping,
                view,
                payload_len,
            })
        }
    }

    /// Copy the newest frame into `dst` (must fit payload). `true` when
    /// a complete published frame was copied — a stale-but-published
    /// frame still counts (latest-wins beats the synthetic fallback).
    pub fn read_frame(&mut self, dst: &mut [u8]) -> bool {
        if dst.len() < self.payload_len {
            return false;
        }
        unsafe {
            for _ in 0..8 {
                let seq1 = read_u32(self.base(), OFF_SEQ);
                if seq1 == 0 {
                    return false; // producer never wrote yet
                }
                if (seq1 & 1) != 0 {
                    std::hint::spin_loop();
                    continue;
                }
                std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
                std::ptr::copy_nonoverlapping(
                    self.base().add(HEADER_BYTES),
                    dst.as_mut_ptr(),
                    self.payload_len,
                );
                std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
                if read_u32(self.base(), OFF_SEQ) == seq1 {
                    write_u32(self.base(), OFF_READ_SEQ, seq1);
                    return true;
                }
            }
            false
        }
    }

    /// Bytes per published frame; 640*480*4 for the advertised format.
    pub fn payload_len(&self) -> usize {
        self.payload_len
    }
}

impl Drop for FrameReader {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self._mapping);
        }
    }
}

/// How often `write_frame` re-resolves the slot name to spot a
/// recycled mapping object (see `FrameWriter::maybe_reconnect`).
const RECONNECT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Result of opening + mapping + validating the named slot.
struct Attach {
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    payload_len: usize,
    width: u32,
    height: u32,
    instance: u32,
}

/// Open + map + validate the named slot. Shared by `FrameWriter::open`
/// and the reconnect probe. Opens with the least rights needed: the
/// DACL grants Everyone GR|GW only, so requesting ALL_ACCESS would be
/// denied.
fn attach(name: &str) -> Result<Attach> {
    unsafe {
        let mapping = OpenFileMappingW(
            FILE_MAP_READ.0 | FILE_MAP_WRITE.0,
            false,
            PCWSTR(wide(name).as_ptr()),
        )?;
        let view = MapViewOfFile(
            mapping,
            FILE_MAP(FILE_MAP_READ.0 | FILE_MAP_WRITE.0),
            0,
            0,
            0,
        );
        if view.Value.is_null() {
            let e = last_error();
            let _ = CloseHandle(mapping);
            return Err(e);
        }
        let base = view.Value as *const u8;
        if read_u32(base, OFF_MAGIC) != MAGIC
            || read_u32(base, OFF_VERSION) != VERSION
            || read_u32(base, OFF_FORMAT) != FORMAT_BGRX
        {
            let _ = UnmapViewOfFile(view);
            let _ = CloseHandle(mapping);
            return Err(E_UNEXPECTED.into());
        }
        Ok(Attach {
            mapping,
            view,
            payload_len: read_u32(base, OFF_PAYLOAD_LEN) as usize,
            width: read_u32(base, OFF_WIDTH),
            height: read_u32(base, OFF_HEIGHT),
            instance: read_u32(base, OFF_INSTANCE),
        })
    }
}

/// Producer side: opens the consumer-created slot and publishes frames.
pub struct FrameWriter {
    _mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    name: String,
    payload_len: usize,
    seq: u32,
    /// OFF_INSTANCE token captured at attach time — the consumer stamps
    /// a fresh one per open, so a mismatch marks a recycled consumer.
    instance: u32,
    last_probe: std::time::Instant,
    reconnects: u32,
    pub width: u32,
    pub height: u32,
}

impl FrameWriter {
    fn base(&self) -> *mut u8 {
        self.view.Value as *mut u8
    }

    /// Attach to the slot. `Err(ERROR_FILE_NOT_FOUND)` while no camera
    /// consumer has created it yet — retry; that is the demand gate.
    pub fn open() -> Result<Self> {
        Self::open_named(MAPPING_NAME)
    }

    /// `open` against an explicit name (test hook — see `FrameReader`).
    fn open_named(name: &str) -> Result<Self> {
        let att = attach(name)?;
        Ok(FrameWriter {
            _mapping: att.mapping,
            view: att.view,
            name: name.to_string(),
            payload_len: att.payload_len,
            seq: 0,
            instance: att.instance,
            last_probe: std::time::Instant::now(),
            reconnects: 0,
            width: att.width,
            height: att.height,
        })
    }

    /// Re-resolve the slot name and compare instance tokens, at most
    /// once per `RECONNECT_INTERVAL`. Called from `write_frame`.
    ///
    /// Rationale: when the FS host recycles, our old mapping object can
    /// be orphaned — a new consumer generation opens the slot while we
    /// keep writing into a dead view. The consumer stamps a fresh
    /// OFF_INSTANCE token on every open, so a token mismatch between
    /// the object we hold and the object the name resolves to now means
    /// "re-attach": unmap the stale view and adopt the current object.
    ///
    /// `Ok(true)` = attached to a new consumer generation. Errors are
    /// folded into `Ok(false)`: if the name is missing or the new object
    /// is half-initialized we keep the old view and retry next tick —
    /// writes into an orphaned object are harmless.
    pub fn maybe_reconnect(&mut self) -> Result<bool> {
        if self.last_probe.elapsed() < RECONNECT_INTERVAL {
            return Ok(false);
        }
        self.last_probe = std::time::Instant::now();
        self.reattach()
    }

    /// The probe half of `maybe_reconnect`, without the time gate
    /// (tests drive it directly).
    fn reattach(&mut self) -> Result<bool> {
        let att = match attach(&self.name) {
            Ok(a) => a,
            Err(_) => return Ok(false),
        };
        if att.instance == self.instance {
            // Same consumer generation — drop the extra view+handle.
            unsafe {
                let _ = UnmapViewOfFile(att.view);
                let _ = CloseHandle(att.mapping);
            }
            return Ok(false);
        }
        // New consumer generation: swap to whatever the name resolves
        // to now (same surviving object or a brand-new mapping).
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self._mapping);
        }
        self._mapping = att.mapping;
        self.view = att.view;
        self.payload_len = att.payload_len;
        self.width = att.width;
        self.height = att.height;
        self.instance = att.instance;
        self.reconnects += 1;
        Ok(true)
    }

    /// Number of consumer-generation changes this writer has followed.
    /// `sadacam-cli feed` logs a line when this advances.
    pub fn reconnect_count(&self) -> u32 {
        self.reconnects
    }

    /// Publish one BGRX frame (exactly `width*height*4` bytes).
    pub fn write_frame(&mut self, bgra: &[u8]) -> Result<()> {
        let _ = self.maybe_reconnect();
        if bgra.len() != self.payload_len {
            return Err(E_INVALIDARG.into());
        }
        unsafe {
            self.seq = self.seq.wrapping_add(1) | 1;
            write_u32(self.base(), OFF_SEQ, self.seq);
            std::ptr::copy_nonoverlapping(
                bgra.as_ptr(),
                self.base().add(HEADER_BYTES),
                self.payload_len,
            );
            std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
            write_u32(self.base(), OFF_SEQ, self.seq.wrapping_add(1));
        }
        Ok(())
    }

    /// Seq of the frame FS last consumed — 0 while nothing pulls.
    pub fn last_read_seq(&self) -> u32 {
        read_u32(self.base() as *const u8, OFF_READ_SEQ)
    }

    /// Consumer-side stage marker (debug): 1 = reader mapped ok.
    pub fn status(&self) -> u32 {
        read_u32(self.base() as *const u8, OFF_STATUS)
    }

    /// Instance token of the mapping we're attached to.
    pub fn instance(&self) -> u32 {
        read_u32(self.base() as *const u8, OFF_INSTANCE)
    }

    /// Producer-side seq currently published.
    pub fn current_seq(&self) -> u32 {
        read_u32(self.base() as *const u8, OFF_SEQ)
    }
}

impl Drop for FrameWriter {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self._mapping);
        }
    }
}

/// Minimal 24-bit BMP decoder → BGRX (top-down). Used by `feed --bmp`.
pub fn decode_bmp24(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    if bytes.len() < 54 || &bytes[0..2] != b"BM" {
        return Err(E_INVALIDARG.into());
    }
    let px_off = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize;
    let w = i32::from_le_bytes(bytes[18..22].try_into().unwrap());
    let h_raw = i32::from_le_bytes(bytes[22..26].try_into().unwrap());
    let bpp = u16::from_le_bytes(bytes[28..30].try_into().unwrap());
    let compression = u32::from_le_bytes(bytes[30..34].try_into().unwrap());
    if bpp != 24 || compression != 0 || w <= 0 || h_raw == 0 {
        return Err(E_INVALIDARG.into());
    }
    let (w, h) = (w as usize, h_raw.unsigned_abs() as usize);
    let top_down = h_raw < 0;
    let row_src = (w * 3 + 3) & !3;
    if px_off + row_src * h > bytes.len() {
        return Err(E_INVALIDARG.into());
    }
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = if top_down { y } else { h - 1 - y };
        let row = &bytes[px_off + sy * row_src..];
        for x in 0..w {
            let (s, d) = (x * 3, (y * w + x) * 4);
            out[d] = row[s]; // B
            out[d + 1] = row[s + 1]; // G
            out[d + 2] = row[s + 2]; // R
            out[d + 3] = 0xFF;
        }
    }
    Ok((w as u32, h as u32, out))
}

// Re-export for embedders wanting the mapping name (diagnostics).
pub fn mapping_name() -> &'static str {
    MAPPING_NAME
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);
    /// Session-private mapping name per test (no `Global\` privilege needed).
    fn uniq_name() -> String {
        format!(
            "SadaCamShmTest_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// While the writer holds its handle the name still resolves to the
    /// SAME object — a recycled consumer can only re-attach to it and
    /// stamp a fresh token. That token change is what `reattach` must
    /// catch (the true "different object under the same name" case needs
    /// the writer's handle gone first, so it can't exist inside a live
    /// writer — the swap path is identical either way).
    #[test]
    fn writer_follows_consumer_recycle() {
        let name = uniq_name();
        let (w, h) = (32u32, 32u32);
        let frame_a = vec![0x5Au8; (w * h * 4) as usize];
        let frame_b = vec![0xA5u8; (w * h * 4) as usize];
        let mut dst = vec![0u8; (w * h * 4) as usize];

        let mut r1 = FrameReader::open_named(&name, w, h).unwrap();
        let mut writer = FrameWriter::open_named(&name).unwrap();
        writer.write_frame(&frame_a).unwrap();
        assert!(r1.read_frame(&mut dst));
        assert_eq!(dst, frame_a);

        // Probe while the same consumer generation owns the slot: no-op.
        assert!(!writer.reattach().unwrap());
        assert_eq!(writer.reconnect_count(), 0);

        // Consumer "recycles": a fresh reader stamps a new token.
        drop(r1);
        let mut r2 = FrameReader::open_named(&name, w, h).unwrap();
        assert!(writer.reattach().unwrap());
        assert_eq!(writer.reconnect_count(), 1);

        // A second probe is a no-op again — token is now in sync.
        assert!(!writer.reattach().unwrap());
        assert_eq!(writer.reconnect_count(), 1);

        // Writes keep flowing to the live consumer.
        writer.write_frame(&frame_b).unwrap();
        assert!(r2.read_frame(&mut dst));
        assert_eq!(dst, frame_b);
    }

    /// Name that resolves to nothing / a slot we can't read: `reattach`
    /// keeps the old view and reports no reconnect.
    #[test]
    fn reattach_survives_unresolvable_name() {
        let name = uniq_name();
        let mut writer = {
            let _r = FrameReader::open_named(&name, 32, 32).unwrap();
            FrameWriter::open_named(&name).unwrap()
        };
        // Point the writer at a name that doesn't exist.
        writer.name = format!("{name}_gone");
        assert!(!writer.reattach().unwrap());
        assert_eq!(writer.reconnect_count(), 0);
        // The writer still works against its original view.
        let frame = vec![7u8; (32 * 32 * 4) as usize];
        writer.write_frame(&frame).unwrap();
    }

    /// `maybe_reconnect` is time-gated: a changed token within the
    /// interval is not picked up until the window elapses.
    #[test]
    fn maybe_reconnect_is_throttled() {
        let name = uniq_name();
        let _r1 = FrameReader::open_named(&name, 32, 32).unwrap();
        let mut writer = FrameWriter::open_named(&name).unwrap();
        // Force the probe window into the past, then verify it probes.
        writer.last_probe = std::time::Instant::now() - RECONNECT_INTERVAL;
        assert!(!writer.maybe_reconnect().unwrap()); // same token → false
        // Consumer recycles inside the just-probed window: not detected yet.
        let _r2 = FrameReader::open_named(&name, 32, 32).unwrap();
        assert!(!writer.maybe_reconnect().unwrap());
        // After the interval it is.
        writer.last_probe = std::time::Instant::now() - RECONNECT_INTERVAL;
        assert!(writer.maybe_reconnect().unwrap());
        assert_eq!(writer.reconnect_count(), 1);
    }
}
