#![allow(non_snake_case)]

mod activate;
pub mod frames;
pub mod overlay;
pub mod register;
pub mod shm;
mod source;
mod stream;
pub mod webcam;

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, Ordering};
use windows::core::*;
use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_POINTER, S_FALSE, S_OK};
use windows::Win32::Media::MediaFoundation::IMFActivate;
use windows::Win32::System::Com::IClassFactory;

// Media-source CLSID for the SadaCraft virtual camera source.
// Frame Server activates this COM class in its host process to pull frames.
pub const CLSID_SADACAM_SOURCE: GUID =
    GUID::from_u128(0x4c8f1b2e_9d3a_4f7c_a6e1_5b0d2c8f4a9eu128);
pub const CLSID_SADACAM_SOURCE_STR: &str = "{4C8F1B2E-9D3A-4F7C-A6E1-5B0D2C8F4A9E}";

static SERVER_LOCK_COUNT: AtomicI32 = AtomicI32::new(0);

pub(crate) fn server_lock() {
    SERVER_LOCK_COUNT.fetch_add(1, Ordering::SeqCst);
}

pub(crate) fn server_unlock() {
    SERVER_LOCK_COUNT.fetch_sub(1, Ordering::SeqCst);
}

#[no_mangle]
extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        if ppv.is_null() {
            return E_POINTER;
        }
        *ppv = std::ptr::null_mut();

        if rclsid.is_null() || *rclsid != CLSID_SADACAM_SOURCE {
            return CLASS_E_CLASSNOTAVAILABLE;
        }

        let factory: IClassFactory = activate::SadaCamFactory {}.into();
        factory.query(riid, ppv)
    }
}

#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    if SERVER_LOCK_COUNT.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[no_mangle]
extern "system" fn DllRegisterServer() -> HRESULT {
    match register::register_server() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

#[no_mangle]
extern "system" fn DllUnregisterServer() -> HRESULT {
    match register::unregister_server() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

/// Test-pattern frame filler shared by the media source fallback and
/// `sadacam-cli feed` (produces the M1 animated color bars).
pub fn fill_test_frame(buf: &mut [u8], width: usize, height: usize, frame_idx: u32) {
    frames::fill_rgb32(buf, width, height, width * 4, frame_idx)
}

// Re-export so external callers (and the CLI) can reach IMFActivate creation directly.
pub fn create_activate() -> Result<IMFActivate> {
    let act: IMFActivate = activate::SadaCamActivate::new()?.into();
    Ok(act)
}
