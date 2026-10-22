use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use windows::core::*;
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_UNEXPECTED, S_OK};
use windows::Win32::Media::KernelStreaming::PINNAME_VIDEO_CAPTURE;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

pub const FRAME_WIDTH: u32 = 640;
pub const FRAME_HEIGHT: u32 = 480;
pub const FRAME_FPS: u32 = 30;
pub const FRAME_DURATION_100NS: i64 = 10_000_000 / FRAME_FPS as i64;
const FRAME_BYTES: u32 = FRAME_WIDTH * FRAME_HEIGHT * 4;
const FRAME_BYTES_NV12: u32 = FRAME_WIDTH * FRAME_HEIGHT * 3 / 2;

#[derive(PartialEq, Clone, Copy)]
pub enum StreamState {
    Stopped,
    Running,
    Paused,
}

/// State shared between the media source and its stream (sibling COM objects;
/// the source holds this Arc to drive stream lifecycle).
pub struct StreamShared {
    pub queue: IMFMediaEventQueue,
    pub descriptor: IMFStreamDescriptor,
    pub attrs: IMFAttributes,
    pub parent: Mutex<Option<IMFMediaSource>>,
    pub state: Mutex<StreamState>,
    pub shutdown: AtomicBool,
    pub frame_idx: AtomicU32,
    /// Shared-memory feed (M2). `None` = no producer ever opened —
    /// retry open every N requests so a later producer is picked up.
    pub feed: Mutex<Option<crate::shm::FrameReader>>,
    pub feed_retry: AtomicU32,
}

impl StreamShared {
    fn check_alive(&self) -> Result<()> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(MF_E_SHUTDOWN.into());
        }
        Ok(())
    }
}

fn create_media_type(subtype: &GUID, frame_bytes: u32) -> Result<IMFMediaType> {
    unsafe {
        let mt = MFCreateMediaType()?;
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        mt.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        mt.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        mt.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        mt.SetUINT64(
            &MF_MT_FRAME_SIZE,
            ((FRAME_WIDTH as u64) << 32) | FRAME_HEIGHT as u64,
        )?;
        mt.SetUINT64(&MF_MT_FRAME_RATE, ((FRAME_FPS as u64) << 32) | 1)?;
        mt.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        mt.SetUINT32(&MF_MT_AVG_BITRATE, frame_bytes * FRAME_FPS * 8)?;
        Ok(mt)
    }
}

pub fn new_stream_shared() -> Result<Arc<StreamShared>> {
    unsafe {
        let queue = MFCreateEventQueue()?;
        // Per-client format table (see routing.rs): the routed preference
        // is advertised first and set as the handler's current type, so
        // clients that never negotiate get it; clients that do call
        // SetCurrentMediaType still get exactly what they asked for.
        // Absent config, RGB32 stays first — the original default.
        let mt_rgb32 = create_media_type(&MFVideoFormat_RGB32, FRAME_BYTES)?;
        let mt_nv12 = create_media_type(&MFVideoFormat_NV12, FRAME_BYTES_NV12)?;
        let (first, second) = match crate::routing::preferred_format() {
            Some(crate::routing::PreferredFormat::Nv12) => (mt_nv12, mt_rgb32),
            _ => (mt_rgb32, mt_nv12),
        };
        let descriptor = MFCreateStreamDescriptor(0, &[Some(first.clone()), Some(second)])?;
        // Make the routed type the handler's current (default) media type.
        let _ = descriptor
            .GetMediaTypeHandler()
            .and_then(|h| h.SetCurrentMediaType(&first));
        // Attributes Frame Server requires on the stream descriptor.
        descriptor.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
        descriptor.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, 0)?;
        descriptor.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
        descriptor.SetUINT32(
            &MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES,
            MFFrameSourceTypes_Color.0 as u32,
        )?;

        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 4)?;
        let attrs = attrs.unwrap();
        attrs.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
        attrs.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, 0)?;
        attrs.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
        attrs.SetUINT32(
            &MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES,
            MFFrameSourceTypes_Color.0 as u32,
        )?;

        Ok(Arc::new(StreamShared {
            queue,
            descriptor,
            attrs,
            parent: Mutex::new(None),
            state: Mutex::new(StreamState::Stopped),
            shutdown: AtomicBool::new(false),
            frame_idx: AtomicU32::new(0),
            feed: Mutex::new(None),
            feed_retry: AtomicU32::new(0),
        }))
    }
}

#[implement(IMFMediaStream2)]
pub struct SadaMediaStream {
    pub(crate) shared: Arc<StreamShared>,
}

impl SadaMediaStream_Impl {
    /// Pull the newest producer frame into `dst`. Re-opens the mapping
    /// ~every 30 requests while no producer is attached (a producer can
    /// arrive after the camera started). `false` → caller draws synthetic.
    fn read_feed(&self, dst: &mut [u8]) -> bool {
        let mut feed = match self.shared.feed.lock() {
            Ok(f) => f,
            Err(_) => return false,
        };
        if feed.is_none() {
            // Cheap retry gate: try opening once per ~30 pull requests.
            let retry = self.shared.feed_retry.fetch_add(1, Ordering::Relaxed);
            if retry % 30 != 0 {
                return false;
            }
            match crate::shm::FrameReader::open(FRAME_WIDTH, FRAME_HEIGHT) {
                Ok(r) => *feed = Some(r),
                Err(_) => return false,
            }
        }
        let reader = feed.as_mut().unwrap();
        // A producer publishing a different frame size can't fill this
        // sample — keep the synthetic pattern.
        if reader.payload_len() != dst.len() {
            return false;
        }
        reader.read_frame(dst)
    }

    /// Subtype the client selected on the stream descriptor's type
    /// handler. RGB32 until anything calls `SetCurrentMediaType` —
    /// clients that never negotiate stay on the original path.
    /// (Format selection happens on the descriptor's IMFMediaTypeHandler,
    /// not on the stream object, so we just read the current type here.)
    fn current_subtype(&self) -> GUID {
        unsafe {
            self.shared
                .descriptor
                .GetMediaTypeHandler()
                .and_then(|h| h.GetCurrentMediaType())
                .and_then(|mt| mt.GetGUID(&MF_MT_SUBTYPE))
                .unwrap_or(MFVideoFormat_RGB32)
        }
    }
}

impl IMFMediaEventGenerator_Impl for SadaMediaStream_Impl {
    fn GetEvent(&self, dwflags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        unsafe { self.shared.queue.GetEvent(dwflags.0 as u32) }
    }
    fn BeginGetEvent(
        &self,
        pcallback: Ref<IMFAsyncCallback>,
        punkstate: Ref<IUnknown>,
    ) -> Result<()> {
        unsafe {
            self.shared
                .queue
                .BeginGetEvent(pcallback.as_ref(), punkstate.as_ref())
        }
    }
    fn EndGetEvent(&self, presult: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        unsafe { self.shared.queue.EndGetEvent(presult.as_ref()) }
    }
    fn QueueEvent(
        &self,
        met: u32,
        guidextendedtype: *const GUID,
        hrstatus: HRESULT,
        pvvalue: *const PROPVARIANT,
    ) -> Result<()> {
        unsafe {
            self.shared
                .queue
                .QueueEventParamVar(met, guidextendedtype, hrstatus, pvvalue)
        }
    }
}

impl IMFMediaStream_Impl for SadaMediaStream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        let guard = self
            .shared
            .parent
            .lock()
            .map_err(|_| Error::from_hresult(E_FAIL))?;
        guard
            .clone()
            .ok_or_else(|| Error::from_hresult(E_UNEXPECTED))
    }

    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        Ok(self.shared.descriptor.clone())
    }

    // Pull-model contract: Frame Server calls RequestSample; the sample must
    // be produced synchronously here (deferred work does not run in the FS host).
    fn RequestSample(&self, ptoken: Ref<IUnknown>) -> Result<()> {
        self.shared.check_alive()?;
        {
            let state = self
                .shared
                .state
                .lock()
                .map_err(|_| Error::from_hresult(E_FAIL))?;
            if *state != StreamState::Running {
                return Err(MF_E_INVALIDREQUEST.into());
            }
        }

        unsafe {
            // NV12 for clients that selected it on the type handler;
            // RGB32 (the default/current type) for everything else.
            let nv12 = self.current_subtype() == MFVideoFormat_NV12;
            let sample_bytes = if nv12 { FRAME_BYTES_NV12 } else { FRAME_BYTES };
            let sample = MFCreateSample()?;
            let buffer = MFCreateMemoryBuffer(sample_bytes)?;
            sample.AddBuffer(&buffer)?;

            let mut pbuf: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut pbuf, None, None)?;
            let idx = self.shared.frame_idx.fetch_add(1, Ordering::SeqCst);
            let dst = std::slice::from_raw_parts_mut(pbuf, sample_bytes as usize);
            if nv12 {
                // Compose in BGRX scratch first (feed or synthetic), then
                // convert. Feed/synthetic memory is bottom-up (RGB32 DIB
                // convention); bgrx_to_nv12 emits top-down planes.
                let mut bgrx = vec![0u8; FRAME_BYTES as usize];
                if !self.read_feed(&mut bgrx) {
                    crate::frames::fill_rgb32(
                        &mut bgrx,
                        FRAME_WIDTH as usize,
                        FRAME_HEIGHT as usize,
                        (FRAME_WIDTH * 4) as usize,
                        idx,
                    );
                }
                crate::frames::bgrx_to_nv12(
                    &bgrx,
                    FRAME_WIDTH as usize,
                    FRAME_HEIGHT as usize,
                    dst,
                    true,
                );
            } else if !self.read_feed(dst) {
                crate::frames::fill_rgb32(
                    dst,
                    FRAME_WIDTH as usize,
                    FRAME_HEIGHT as usize,
                    (FRAME_WIDTH * 4) as usize,
                    idx,
                );
            }
            buffer.Unlock()?;
            buffer.SetCurrentLength(sample_bytes)?;

            sample.SetSampleTime(MFGetSystemTime())?;
            sample.SetSampleDuration(FRAME_DURATION_100NS)?;
            if let Some(token) = ptoken.as_ref() {
                sample.SetUnknown(&MFSampleExtension_Token, token)?;
            }
            self.shared.queue.QueueEventParamUnk(
                MEMediaSample.0 as u32,
                &GUID::zeroed(),
                S_OK,
                &sample,
            )?;
        }
        Ok(())
    }
}

impl IMFMediaStream2_Impl for SadaMediaStream_Impl {
    fn SetStreamState(&self, value: MF_STREAM_STATE) -> Result<()> {
        self.shared.check_alive()?;
        let new = if value == MF_STREAM_STATE_RUNNING {
            StreamState::Running
        } else if value == MF_STREAM_STATE_PAUSED {
            StreamState::Paused
        } else if value == MF_STREAM_STATE_STOPPED {
            StreamState::Stopped
        } else {
            return Err(E_INVALIDARG.into());
        };
        {
            let mut state = self
                .shared
                .state
                .lock()
                .map_err(|_| Error::from_hresult(E_FAIL))?;
            if *state == new {
                return Ok(());
            }
            *state = new;
        }
        let empty = PROPVARIANT::default();
        unsafe {
            match new {
                StreamState::Running => self.shared.queue.QueueEventParamVar(
                    MEStreamStarted.0 as u32,
                    &GUID::zeroed(),
                    S_OK,
                    &empty,
                ),
                _ => self.shared.queue.QueueEventParamVar(
                    MEStreamStopped.0 as u32,
                    &GUID::zeroed(),
                    S_OK,
                    &empty,
                ),
            }
        }
    }

    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        self.shared.check_alive()?;
        let state = self
            .shared
            .state
            .lock()
            .map_err(|_| Error::from_hresult(E_FAIL))?;
        Ok(match *state {
            StreamState::Running => MF_STREAM_STATE_RUNNING,
            StreamState::Paused => MF_STREAM_STATE_PAUSED,
            StreamState::Stopped => MF_STREAM_STATE_STOPPED,
        })
    }
}
