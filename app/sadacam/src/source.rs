use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows::core::*;
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOTIMPL, S_OK};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

use crate::stream::{new_stream_shared, SadaMediaStream, StreamShared};

struct SourceState {
    shutdown: bool,
}

/// Pull-model custom media source loaded by Windows Frame Server in its host
/// process. Implements IMFMediaSourceEx (the extension FS requires over plain
/// IMFMediaSource) plus IMFGetService (must return MF_E_UNSUPPORTED_SERVICE).
#[implement(IMFMediaSourceEx, IMFGetService)]
pub struct SadaMediaSource {
    queue: IMFMediaEventQueue,
    pd: IMFPresentationDescriptor,
    attrs: IMFAttributes,
    stream: IMFMediaStream2,
    shared: Arc<StreamShared>,
    state: Mutex<SourceState>,
    announced: AtomicBool,
}

impl SadaMediaSource {
    pub fn new() -> Result<Self> {
        unsafe {
            let queue = MFCreateEventQueue()?;
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 0)?;

            let shared = new_stream_shared()?;
            let stream: IMFMediaStream2 = SadaMediaStream {
                shared: shared.clone(),
            }
            .into();

            let pd = MFCreatePresentationDescriptor(Some(&[Some(shared.descriptor.clone())]))?;
            pd.SelectStream(0)?;

            Ok(Self {
                queue,
                pd,
                attrs: attrs.unwrap(),
                stream,
                shared,
                state: Mutex::new(SourceState { shutdown: false }),
                announced: AtomicBool::new(false),
            })
        }
    }

    fn check_alive(&self) -> Result<()> {
        let state = self.state.lock().map_err(|_| Error::from_hresult(E_FAIL))?;
        if state.shutdown {
            return Err(MF_E_SHUTDOWN.into());
        }
        Ok(())
    }
}

impl IMFMediaEventGenerator_Impl for SadaMediaSource_Impl {
    fn GetEvent(&self, dwflags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        unsafe { self.queue.GetEvent(dwflags.0 as u32) }
    }
    fn BeginGetEvent(&self, pcallback: Ref<IMFAsyncCallback>, punkstate: Ref<IUnknown>) -> Result<()> {
        unsafe {
            self.queue
                .BeginGetEvent(pcallback.as_ref(), punkstate.as_ref())
        }
    }
    fn EndGetEvent(&self, presult: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        unsafe { self.queue.EndGetEvent(presult.as_ref()) }
    }
    fn QueueEvent(
        &self,
        met: u32,
        guidextendedtype: *const GUID,
        hrstatus: HRESULT,
        pvvalue: *const PROPVARIANT,
    ) -> Result<()> {
        unsafe { self.queue.QueueEventParamVar(met, guidextendedtype, hrstatus, pvvalue) }
    }
}

impl IMFMediaSource_Impl for SadaMediaSource_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }

    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        self.check_alive()?;
        unsafe { self.pd.Clone() }
    }

    fn Start(
        &self,
        _ppresentationdescriptor: Ref<IMFPresentationDescriptor>,
        _pguidtimeformat: *const GUID,
        _pvarstartposition: *const PROPVARIANT,
    ) -> Result<()> {
        self.check_alive()?;

        // Give the stream its parent back-reference on first start.
        {
            let mut parent = self
                .shared
                .parent
                .lock()
                .map_err(|_| Error::from_hresult(E_FAIL))?;
            if parent.is_none() {
                let ex: IMFMediaSourceEx = self.to_interface();
                *parent = Some(ex.into());
            }
        }

        let empty = PROPVARIANT::default();
        unsafe {
            let announce = if !self.announced.swap(true, Ordering::SeqCst) {
                MENewStream
            } else {
                MEUpdatedStream
            };
            self.queue.QueueEventParamUnk(
                announce.0 as u32,
                &GUID::zeroed(),
                S_OK,
                &self.stream,
            )?;
            self.queue.QueueEventParamVar(
                MESourceStarted.0 as u32,
                &GUID::zeroed(),
                S_OK,
                &empty,
            )?;
        }

        // Move the stream to running; fires MEStreamStarted on the stream queue.
        unsafe { self.stream.SetStreamState(MF_STREAM_STATE_RUNNING) }
    }

    fn Stop(&self) -> Result<()> {
        self.check_alive()?;
        let _ = unsafe { self.stream.SetStreamState(MF_STREAM_STATE_STOPPED) };
        let empty = PROPVARIANT::default();
        unsafe {
            self.queue.QueueEventParamVar(
                MESourceStopped.0 as u32,
                &GUID::zeroed(),
                S_OK,
                &empty,
            )?;
        }
        Ok(())
    }

    fn Pause(&self) -> Result<()> {
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }

    fn Shutdown(&self) -> Result<()> {
        {
            let mut state = self.state.lock().map_err(|_| Error::from_hresult(E_FAIL))?;
            if state.shutdown {
                return Ok(());
            }
            state.shutdown = true;
        }
        self.shared.shutdown.store(true, Ordering::SeqCst);
        unsafe {
            let _ = self.shared.queue.Shutdown();
            let _ = self.queue.Shutdown();
        }
        Ok(())
    }
}

impl IMFMediaSourceEx_Impl for SadaMediaSource_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        Ok(self.attrs.clone())
    }
    fn GetStreamAttributes(&self, dwstreamidentifier: u32) -> Result<IMFAttributes> {
        if dwstreamidentifier != 0 {
            return Err(E_INVALIDARG.into());
        }
        Ok(self.shared.attrs.clone())
    }
    fn SetD3DManager(&self, _pmanager: Ref<IUnknown>) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}

impl IMFGetService_Impl for SadaMediaSource_Impl {
    // Frame Server contract: all services unsupported.
    fn GetService(
        &self,
        _guidservice: *const GUID,
        _riid: *const GUID,
        _ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}
