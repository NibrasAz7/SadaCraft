use std::ffi::c_void;
use std::sync::Mutex;
use windows::core::*;
use windows::Win32::Foundation::{
    CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};

#[implement(IClassFactory)]
pub struct SadaCamFactory;

impl IClassFactory_Impl for SadaCamFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        unsafe {
            if ppvobject.is_null() {
                return Err(E_POINTER.into());
            }
            *ppvobject = std::ptr::null_mut();
            if !punkouter.is_null() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let activate: IMFActivate = SadaCamActivate::new()?.into();
            let hr = activate.query(riid, ppvobject);
            if hr.is_err() {
                return Err(Error::from_hresult(hr));
            }
            crate::server_lock();
            Ok(())
        }
    }

    fn LockServer(&self, _flock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// Activation object Frame Server instantiates via DllGetClassObject ->
/// IClassFactory -> IMFActivate. Attributes delegate to an inner
/// MFCreateAttributes store; ActivateObject produces the media source.
#[implement(IMFActivate)]
pub struct SadaCamActivate {
    attrs: IMFAttributes,
    source: Mutex<Option<IMFMediaSource>>,
}

impl SadaCamActivate {
    pub fn new() -> Result<Self> {
        let mut attrs: Option<IMFAttributes> = None;
        unsafe { MFCreateAttributes(&mut attrs, 1)? };
        Ok(Self {
            attrs: attrs.unwrap(),
            source: Mutex::new(None),
        })
    }
}

impl Drop for SadaCamActivate {
    fn drop(&mut self) {
        crate::server_unlock();
    }
}

impl IMFActivate_Impl for SadaCamActivate_Impl {
    fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        unsafe {
            if ppv.is_null() {
                return Err(E_POINTER.into());
            }
            *ppv = std::ptr::null_mut();
            let mut guard = self.source.lock().map_err(|_| Error::from_hresult(E_FAIL))?;
            if guard.is_none() {
                let ex: IMFMediaSourceEx = crate::source::SadaMediaSource::new()?.into();
                *guard = Some(ex.into());
            }
            let source = guard.as_ref().unwrap();
            let hr = source.query(riid, ppv);
            if hr.is_err() {
                Err(Error::from_hresult(hr))
            } else {
                Ok(())
            }
        }
    }

    fn ShutdownObject(&self) -> Result<()> {
        let guard = self.source.lock().map_err(|_| Error::from_hresult(E_FAIL))?;
        if let Some(source) = guard.as_ref() {
            unsafe { source.Shutdown() }
        } else {
            Ok(())
        }
    }

    fn DetachObject(&self) -> Result<()> {
        let mut guard = self.source.lock().map_err(|_| Error::from_hresult(E_FAIL))?;
        *guard = None;
        Ok(())
    }
}

impl IMFAttributes_Impl for SadaCamActivate_Impl {
    fn GetItem(&self, guidkey: *const GUID, pvalue: *mut PROPVARIANT) -> Result<()> {
        unsafe { self.attrs.GetItem(guidkey, Some(pvalue)) }
    }
    fn GetItemType(&self, guidkey: *const GUID) -> Result<MF_ATTRIBUTE_TYPE> {
        unsafe { self.attrs.GetItemType(guidkey) }
    }
    fn CompareItem(&self, guidkey: *const GUID, value: *const PROPVARIANT) -> Result<BOOL> {
        unsafe { self.attrs.CompareItem(guidkey, value) }
    }
    fn Compare(
        &self,
        ptheirs: Ref<IMFAttributes>,
        matchtype: MF_ATTRIBUTES_MATCH_TYPE,
    ) -> Result<BOOL> {
        unsafe { self.attrs.Compare(ptheirs.as_ref(), matchtype) }
    }
    fn GetUINT32(&self, guidkey: *const GUID) -> Result<u32> {
        unsafe { self.attrs.GetUINT32(guidkey) }
    }
    fn GetUINT64(&self, guidkey: *const GUID) -> Result<u64> {
        unsafe { self.attrs.GetUINT64(guidkey) }
    }
    fn GetDouble(&self, guidkey: *const GUID) -> Result<f64> {
        unsafe { self.attrs.GetDouble(guidkey) }
    }
    fn GetGUID(&self, guidkey: *const GUID) -> Result<GUID> {
        unsafe { self.attrs.GetGUID(guidkey) }
    }
    fn GetStringLength(&self, guidkey: *const GUID) -> Result<u32> {
        unsafe { self.attrs.GetStringLength(guidkey) }
    }
    fn GetString(
        &self,
        guidkey: *const GUID,
        pwszvalue: PWSTR,
        cchbufsize: u32,
        pcchlength: *mut u32,
    ) -> Result<()> {
        unsafe {
            self.attrs.GetString(
                guidkey,
                std::slice::from_raw_parts_mut(pwszvalue.0, cchbufsize as usize),
                Some(pcchlength),
            )
        }
    }
    fn GetAllocatedString(
        &self,
        guidkey: *const GUID,
        ppwszvalue: *mut PWSTR,
        pcchlength: *mut u32,
    ) -> Result<()> {
        unsafe { self.attrs.GetAllocatedString(guidkey, ppwszvalue, pcchlength) }
    }
    fn GetBlobSize(&self, guidkey: *const GUID) -> Result<u32> {
        unsafe { self.attrs.GetBlobSize(guidkey) }
    }
    fn GetBlob(
        &self,
        guidkey: *const GUID,
        pbuf: *mut u8,
        cbbufsize: u32,
        pcbblobsize: *mut u32,
    ) -> Result<()> {
        unsafe {
            self.attrs.GetBlob(
                guidkey,
                std::slice::from_raw_parts_mut(pbuf, cbbufsize as usize),
                Some(pcbblobsize),
            )
        }
    }
    fn GetAllocatedBlob(
        &self,
        guidkey: *const GUID,
        ppbuf: *mut *mut u8,
        pcbsize: *mut u32,
    ) -> Result<()> {
        unsafe { self.attrs.GetAllocatedBlob(guidkey, ppbuf, pcbsize) }
    }
    fn GetUnknown(&self, guidkey: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        unsafe {
            let unk: IUnknown = self.attrs.GetUnknown(guidkey)?;
            let hr = unk.query(riid, ppv);
            if hr.is_err() {
                Err(Error::from_hresult(hr))
            } else {
                Ok(())
            }
        }
    }
    fn SetItem(&self, guidkey: *const GUID, value: *const PROPVARIANT) -> Result<()> {
        unsafe { self.attrs.SetItem(guidkey, value) }
    }
    fn DeleteItem(&self, guidkey: *const GUID) -> Result<()> {
        unsafe { self.attrs.DeleteItem(guidkey) }
    }
    fn DeleteAllItems(&self) -> Result<()> {
        unsafe { self.attrs.DeleteAllItems() }
    }
    fn SetUINT32(&self, guidkey: *const GUID, unvalue: u32) -> Result<()> {
        unsafe { self.attrs.SetUINT32(guidkey, unvalue) }
    }
    fn SetUINT64(&self, guidkey: *const GUID, unvalue: u64) -> Result<()> {
        unsafe { self.attrs.SetUINT64(guidkey, unvalue) }
    }
    fn SetDouble(&self, guidkey: *const GUID, fvalue: f64) -> Result<()> {
        unsafe { self.attrs.SetDouble(guidkey, fvalue) }
    }
    fn SetGUID(&self, guidkey: *const GUID, guidvalue: *const GUID) -> Result<()> {
        unsafe { self.attrs.SetGUID(guidkey, guidvalue) }
    }
    fn SetString(&self, guidkey: *const GUID, wszvalue: &PCWSTR) -> Result<()> {
        unsafe { self.attrs.SetString(guidkey, *wszvalue) }
    }
    fn SetBlob(&self, guidkey: *const GUID, pbuf: *const u8, cbbufsize: u32) -> Result<()> {
        unsafe {
            self.attrs
                .SetBlob(guidkey, std::slice::from_raw_parts(pbuf, cbbufsize as usize))
        }
    }
    fn SetUnknown(&self, guidkey: *const GUID, punknown: Ref<IUnknown>) -> Result<()> {
        unsafe { self.attrs.SetUnknown(guidkey, punknown.as_ref()) }
    }
    fn LockStore(&self) -> Result<()> {
        unsafe { self.attrs.LockStore() }
    }
    fn UnlockStore(&self) -> Result<()> {
        unsafe { self.attrs.UnlockStore() }
    }
    fn GetCount(&self) -> Result<u32> {
        unsafe { self.attrs.GetCount() }
    }
    fn GetItemByIndex(
        &self,
        unindex: u32,
        pguidkey: *mut GUID,
        pvalue: *mut PROPVARIANT,
    ) -> Result<()> {
        unsafe { self.attrs.GetItemByIndex(unindex, pguidkey, Some(pvalue)) }
    }
    fn CopyAllItems(&self, pdest: Ref<IMFAttributes>) -> Result<()> {
        unsafe { self.attrs.CopyAllItems(pdest.as_ref()) }
    }
}
