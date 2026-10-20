use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use windows::core::*;
use windows::Win32::Foundation::{E_FAIL, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW};
use windows::Win32::System::Registry::*;

use crate::CLSID_SADACAM_SOURCE_STR;

const HKCU_CLASSES_ROOT: &str = "Software\\Classes\\";

fn clsid_key() -> Vec<u16> {
    wide(&format!(
        "{}CLSID\\{}",
        HKCU_CLASSES_ROOT, CLSID_SADACAM_SOURCE_STR
    ))
}

fn inproc_key() -> Vec<u16> {
    wide(&format!(
        "{}CLSID\\{}\\InprocServer32",
        HKCU_CLASSES_ROOT, CLSID_SADACAM_SOURCE_STR
    ))
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn hr_from(status: WIN32_ERROR) -> Error {
    Error::from_hresult(HRESULT::from_win32(status.0))
}

unsafe fn dll_self_path() -> Result<Vec<u16>> {
    let mut hmodule = windows::Win32::Foundation::HMODULE::default();
    GetModuleHandleExW(
        windows::Win32::System::LibraryLoader::GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
            | windows::Win32::System::LibraryLoader::GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        PCWSTR(dll_self_path as *const () as *const u16),
        &mut hmodule,
    )?;
    let mut path = [0u16; 1024];
    let len = GetModuleFileNameW(Some(hmodule), &mut path);
    if len == 0 {
        return Err(Error::from_hresult(E_FAIL));
    }
    let mut v = path[..len as usize].to_vec();
    v.push(0);
    Ok(v)
}

unsafe fn set_sz(hkey: HKEY, name: &PCWSTR, value: &[u16]) -> Result<()> {
    let bytes =
        std::slice::from_raw_parts(value.as_ptr() as *const u8, value.len() * 2);
    let status = RegSetValueExW(hkey, *name, None, REG_SZ, Some(bytes));
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(hr_from(status))
    }
}

/// Per-user COM registration (HKCU).
/// `dll_path` is the full path to sadacam.dll; `DllRegisterServer` passes the
/// self-resolved module path, tools pass an explicit one.
pub fn register_server_at(dll_path: Vec<u16>) -> Result<()> {
    register_server_root(HKEY_CURRENT_USER, dll_path)
}

/// Frame Server runs as LocalService and cannot see per-user (HKCU)
/// registrations: system-wide HKLM registration is required and needs
/// an elevated process.
pub fn register_server_system_at(dll_path: Vec<u16>) -> Result<()> {
    register_server_root(HKEY_LOCAL_MACHINE, dll_path)
}

fn register_server_root(root: HKEY, dll_path: Vec<u16>) -> Result<()> {
    unsafe {
        let mut hkey = HKEY::default();
        let status =
            RegCreateKeyW(root, PCWSTR(clsid_key().as_ptr()), &mut hkey);
        if status != ERROR_SUCCESS {
            return Err(hr_from(status));
        }
        set_sz(hkey, &PCWSTR::null(), &wide("SadaCam Media Source"))?;
        let _ = RegCloseKey(hkey);

        let mut hkey = HKEY::default();
        let status =
            RegCreateKeyW(root, PCWSTR(inproc_key().as_ptr()), &mut hkey);
        if status != ERROR_SUCCESS {
            return Err(hr_from(status));
        }
        set_sz(hkey, &PCWSTR::null(), &dll_path)?;
        set_sz(
            hkey,
            &PCWSTR(wide("ThreadingModel").as_ptr()),
            &wide("Both"),
        )?;
        let _ = RegCloseKey(hkey);
        Ok(())
    }
}

pub fn register_server() -> Result<()> {
    unsafe { register_server_at(dll_self_path()?) }
}

pub fn unregister_server() -> Result<()> {
    unregister_server_root(HKEY_CURRENT_USER)
}

pub fn unregister_server_system() -> Result<()> {
    unregister_server_root(HKEY_LOCAL_MACHINE)
}

fn unregister_server_root(root: HKEY) -> Result<()> {
    unsafe {
        let status = RegDeleteTreeW(root, PCWSTR(clsid_key().as_ptr()));
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(hr_from(status))
        }
    }
}

/// Create + start a session-lifetime virtual camera named `friendly_name`
/// backed by this DLL's media-source CLSID.
pub fn create_camera(friendly_name: &str) -> Result<IMFVirtualCamera> {
    unsafe {
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let name = wide(friendly_name);
        let source_id = wide(CLSID_SADACAM_SOURCE_STR);
        MFCreateVirtualCamera(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_Session,
            MFVirtualCameraAccess_CurrentUser,
            PCWSTR(name.as_ptr()),
            PCWSTR(source_id.as_ptr()),
            None,
        )
    }
}

/// Frame Server lifecycle events for the virtual camera arrive on this
/// callback (SOURCE_START/SOURCE_STOP/ERROR) in the registering process.
#[implement(IMFAsyncCallback)]
pub struct VcamEventsCallback;

impl IMFAsyncCallback_Impl for VcamEventsCallback_Impl {
    fn GetParameters(&self, pdwflags: *mut u32, pdwqueue: *mut u32) -> Result<()> {
        unsafe {
            *pdwflags = 0;
            *pdwqueue = MFASYNC_CALLBACK_QUEUE_MULTITHREADED;
        }
        Ok(())
    }
    fn Invoke(&self, pasyncresult: Ref<IMFAsyncResult>) -> Result<()> {
        let _ = pasyncresult;
        Ok(())
    }
}

pub fn start_camera(vcam: &IMFVirtualCamera) -> Result<()> {
    unsafe {
        let cb: IMFAsyncCallback = VcamEventsCallback.into();
        vcam.Start(&cb)
    }
}

pub fn remove_camera(vcam: &IMFVirtualCamera) -> Result<()> {
    unsafe {
        let _ = vcam.Stop();
        vcam.Remove()
    }
}
