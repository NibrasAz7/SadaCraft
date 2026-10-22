//! Per-client preferred media-type routing (M3), config-driven.
//!
//! `%PROGRAMDATA%\SadaCraft\format-routing.json` (fallback:
//! `format-routing.json` next to the loaded module) maps a client
//! process name to the media type it should prefer:
//!
//! ```json
//! { "zoom.exe": "nv12", "obs64.exe": "rgb32", "*": "nv12" }
//! ```
//!
//! `"*"` is the wildcard default for clients not listed (or when no client
//! name could be detected at all).
//!
//! **Honest limit — client-name detection.** Windows Frame Server activates
//! this COM media source inside its own service host (LocalService); the
//! documented IMFMediaSourceEx / IMFActivate contract carries no client
//! identity. There is no documented user-mode API for the source to learn
//! which app connected. What this module does instead:
//!
//!   1. `SADACAM_CLIENT_NAME` env override (testing / explicit control).
//!   2. Opportunistic probe: any string attribute the Frame Server may
//!      have set on our IMFActivate is scanned for a `*.exe` name
//!      (undocumented channel — usually finds nothing).
//!   3. The `"*"` wildcard entry, which always applies.
//!
//! The preference is applied at stream-descriptor creation: the preferred
//! type is advertised first and made the handler's current media type, so
//! clients that never negotiate get it by default while clients that do
//! call `SetCurrentMediaType` still get exactly what they asked for.
//! Per-format payloads (RGB32 vs NV12) are already produced from the same
//! composed BGRX frame in `stream::RequestSample`.

#![cfg(target_os = "windows")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use windows::core::*;
use windows::Win32::Media::MediaFoundation::IMFAttributes;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT};

/// Media-type preference from the routing table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreferredFormat {
    Rgb32,
    Nv12,
}

static TABLE: OnceLock<BTreeMap<String, PreferredFormat>> = OnceLock::new();
static DETECTED: OnceLock<Option<String>> = OnceLock::new();

/// Path of the loaded module (works for both the exe and sadacam.dll inside
/// the Frame Server host — `GetModuleHandleExW(FROM_ADDRESS)`).
fn module_dir() -> Option<PathBuf> {
    unsafe {
        let mut hmodule = windows::Win32::Foundation::HMODULE::default();
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(module_dir as *const () as *const u16),
            &mut hmodule,
        )
        .ok()?;
        let mut path = [0u16; 1024];
        let len = GetModuleFileNameW(Some(hmodule), &mut path);
        if len == 0 {
            return None;
        }
        let s = String::from_utf16_lossy(&path[..len as usize]);
        PathBuf::from(s).parent().map(|d| d.to_path_buf())
    }
}

/// Config search order: ProgramData first (system-wide, what `install`
/// stages), then the module's own directory.
fn config_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("PROGRAMDATA") {
        v.push(PathBuf::from(p).join("SadaCraft").join("format-routing.json"));
    }
    if let Some(dir) = module_dir() {
        v.push(dir.join("format-routing.json"));
    }
    v
}

fn parse_table(text: &str) -> BTreeMap<String, PreferredFormat> {
    let mut map = BTreeMap::new();
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(text) else {
        return map;
    };
    for (k, v) in obj {
        let Some(s) = v.as_str() else { continue };
        let fmt = match s.to_ascii_lowercase().as_str() {
            "rgb32" | "rgba" | "bgrx" => PreferredFormat::Rgb32,
            "nv12" => PreferredFormat::Nv12,
            _ => continue, // unknown format string — skip the entry
        };
        map.insert(k.to_ascii_lowercase(), fmt);
    }
    map

}

fn table() -> &'static BTreeMap<String, PreferredFormat> {
    TABLE.get_or_init(|| {
        for p in config_paths() {
            if let Ok(text) = std::fs::read_to_string(&p) {
                return parse_table(&text);
            }
        }
        BTreeMap::new()
    })
}

/// Record a client process name discovered by an opportunistic probe
/// (first write wins — later detections are ignored).
pub fn note_detected_client(name: String) {
    let _ = DETECTED.set(Some(name));
}

fn detected_client() -> Option<String> {
    DETECTED
        .get_or_init(|| {
            // Escape hatch for testing / explicit control.
            std::env::var("SADACAM_CLIENT_NAME")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .clone()
}

/// The format this client should prefer: exact process-name match, else
/// the `"*"` wildcard, else `None` (keep RGB32-first built-in order).
pub fn preferred_format() -> Option<PreferredFormat> {
    let t = table();
    if t.is_empty() {
        return None;
    }
    if let Some(client) = detected_client() {
        let name = client.to_ascii_lowercase();
        if let Some(f) = t.get(&name) {
            return Some(*f);
        }
        // Also try just the file name if a path was detected.
        if let Some(base) = name.rsplit(['\\', '/']).next() {
            if let Some(f) = t.get(base) {
                return Some(*f);
            }
        }
    }
    t.get("*").copied()
}

/// Opportunistic client-identity probe: scan every attribute set on our
/// IMFActivate for string values containing a `*.exe` file name. Frame
/// Server is not documented to set any such attribute, so this usually
/// finds nothing — it exists so that if a future host does pass context,
/// routing picks it up without code changes.
pub fn probe_client_identity(attrs: &IMFAttributes) -> Option<String> {
    unsafe {
        let count = attrs.GetCount().ok()?;
        for i in 0..count {
            let mut key = GUID::zeroed();
            let mut pv = PROPVARIANT::default();
            if attrs.GetItemByIndex(i, &mut key, Some(&mut pv)).is_err() {
                continue;
            }
            // GetAllocatedString fails for non-string values — that doubles
            // as the type filter (no PROPVARIANT decoding needed).
            let mut p = PWSTR::null();
            let mut len = 0u32;
            if attrs.GetAllocatedString(&key, &mut p, &mut len).is_err() || p.is_null() {
                continue;
            }
            let s = PCWSTR(p.0 as *const u16).to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
            if let Some(name) = exe_name(&s) {
                return Some(name);
            }
        }
        None
    }
}

/// Extract an executable file name ("zoom.exe") from a string if it
/// mentions one (e.g. a full path or a bare name).
fn exe_name(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    if !lower.ends_with(".exe") {
        return None;
    }
    let base = s.rsplit(['\\', '/']).next().unwrap_or(s);
    Some(base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_routing_json() {
        let t = parse_table(
            r#"{"zoom.exe":"nv12","OBS64.EXE":"RGB32","bad.exe":"h264","*":"nv12"}"#,
        );
        assert_eq!(t.get("zoom.exe"), Some(&PreferredFormat::Nv12));
        assert_eq!(t.get("obs64.exe"), Some(&PreferredFormat::Rgb32));
        assert_eq!(t.get("*"), Some(&PreferredFormat::Nv12));
        assert!(!t.contains_key("bad.exe"));
    }

    #[test]
    fn exe_name_extraction() {
        assert_eq!(
            exe_name(r"C:\Program Files\Zoom\zoom.exe"),
            Some("zoom.exe".to_string())
        );
        assert_eq!(exe_name("obs64.exe"), Some("obs64.exe".to_string()));
        assert_eq!(exe_name("not-an-exe.dll"), None);
    }
}
