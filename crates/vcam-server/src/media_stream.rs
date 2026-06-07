use std::sync::{Arc, Mutex};

use windows::core::{implement, Error, GUID, IUnknown, Interface, Result};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Media::KernelStreaming::PINNAME_VIDEO_CAPTURE;
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMF2DBuffer2, IMFAsyncCallback, IMFAsyncResult, IMFAttributes, IMFMediaBuffer,
    IMFMediaEvent, IMFMediaEventGenerator_Impl, IMFMediaSource, IMFMediaStream, IMFMediaStream2,
    IMFMediaStream2_Impl, IMFMediaStream_Impl, IMFMediaType, IMFMediaTypeHandler, IMFSample,
    IMFStreamDescriptor, IMFVideoSampleAllocator, MF2DBuffer_LockFlags_Write, MFCreateAttributes,
    MFCreateEventQueue, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateStreamDescriptor, MFGetSystemTime, MFMediaType_Video, MFVideoFormat_NV12,
    MFVideoFormat_RGB32, MFVideoInterlace_Progressive, MFSampleExtension_Token,
    MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS, MEEndOfPresentation, MEMediaSample, MEStreamStarted,
    MEStreamStopped, MFFrameSourceTypes_Color, MF_E_INVALIDREQUEST, MF_E_SHUTDOWN,
    MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE,
    MF_MT_FIXED_SIZE_SAMPLES, MF_MT_FRAME_RATE, MF_MT_FRAME_RATE_RANGE_MAX,
    MF_MT_FRAME_RATE_RANGE_MIN, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SAMPLE_SIZE, MF_MT_SUBTYPE, MF_STREAM_STATE,
    MF_STREAM_STATE_RUNNING, MF_STREAM_STATE_STOPPED,
    MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MF_DEVICESTREAM_FRAMESERVER_SHARED,
    MF_DEVICESTREAM_STREAM_CATEGORY, MF_DEVICESTREAM_STREAM_ID,
};
use windows_core::PROPVARIANT;

use crate::constants::STREAM_ID;
use crate::debug_log;
use crate::feed_shared::{FeedFrame, FeedSessionReader};
use crate::media_event::CustomMediaEvent;
use crate::test_pattern::{
    bgra_to_nv12_bytes, copy_bgra_to_surface, copy_nv12_to_surface, StaticTestPattern,
};
use crate::video_format::VideoFormat;

#[derive(Clone)]
pub struct SourceReference {
    iface: Arc<Mutex<Option<IMFMediaSource>>>,
}

impl SourceReference {
    pub fn new() -> Self {
        Self {
            iface: Arc::new(Mutex::new(None)),
        }
    }

    pub fn bind(&self, source: IMFMediaSource) {
        *self.iface.lock().expect("source reference poisoned") = Some(source);
    }

    pub fn get(&self) -> Result<IMFMediaSource> {
        self.iface
            .lock()
            .expect("source reference poisoned")
            .clone()
            .ok_or_else(|| E_POINTER.into())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct StreamState {
    stream_state: MF_STREAM_STATE,
    shutdown: bool,
}

#[derive(Clone)]
struct SampleAllocatorState {
    allocator: Option<IMFVideoSampleAllocator>,
}

#[derive(Clone)]
struct ProvidedFrame {
    frame_id: u64,
    sample_time_100ns: Option<i64>,
    bgra: Arc<[u8]>,
    nv12: Option<Arc<[u8]>>,
}

#[derive(Clone)]
struct StaticPatternProvider {
    pattern: StaticTestPattern,
}

impl StaticPatternProvider {
    fn new(format: VideoFormat) -> Self {
        Self {
            pattern: StaticTestPattern::with_format(format),
        }
    }

    fn current_frame(&self) -> ProvidedFrame {
        ProvidedFrame {
            frame_id: 0,
            sample_time_100ns: None,
            bgra: self.pattern.rgb32_arc(),
            nv12: Some(self.pattern.nv12_arc()),
        }
    }
}

pub(crate) struct SharedMemoryFeedProvider {
    reader: FeedSessionReader,
    last_successful: Mutex<Option<FeedFrame>>,
}

impl SharedMemoryFeedProvider {
    fn open_active() -> Result<Option<Self>> {
        Ok(FeedSessionReader::open_active()?.map(|reader| Self {
            reader,
            last_successful: Mutex::new(None),
        }))
    }

    fn open_matching(format: VideoFormat) -> Result<Option<Self>> {
        let Some(provider) = Self::open_active()? else {
            return Ok(None);
        };
        if provider.format() == format {
            Ok(Some(provider))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn format(&self) -> VideoFormat {
        self.reader
            .config()
            .video_format()
            .expect("validated feed session config must produce a valid video format")
    }

    fn is_active(&self) -> bool {
        self.reader.is_active()
    }

    fn try_current_frame(&self) -> Result<Option<ProvidedFrame>> {
        if !self.reader.is_active() {
            self.last_successful
                .lock()
                .expect("shared provider cache poisoned")
                .take();
            return Ok(None);
        }

        if let Some(frame) = self.reader.try_read_latest_frame()? {
            *self
                .last_successful
                .lock()
                .expect("shared provider cache poisoned") = Some(frame.clone());
            return Ok(Some(ProvidedFrame {
                frame_id: frame.frame_id,
                sample_time_100ns: Some(frame.timestamp_100ns),
                bgra: frame.bgra,
                nv12: None,
            }));
        }

        let cached = self
            .last_successful
            .lock()
            .expect("shared provider cache poisoned")
            .clone();
        Ok(cached.map(|frame| ProvidedFrame {
            frame_id: frame.frame_id,
            sample_time_100ns: Some(frame.timestamp_100ns),
            bgra: frame.bgra,
            nv12: None,
        }))
    }
}

struct FrameProviders {
    static_provider: StaticPatternProvider,
    stream_format: VideoFormat,
    shared_provider: Mutex<Option<SharedMemoryFeedProvider>>,
}

impl FrameProviders {
    fn new(format: VideoFormat, shared_provider: Option<SharedMemoryFeedProvider>) -> Self {
        Self {
            static_provider: StaticPatternProvider::new(format),
            stream_format: format,
            shared_provider: Mutex::new(shared_provider),
        }
    }

    fn current_frame(&self) -> Result<ProvidedFrame> {
        if let Some(frame) = self.try_shared_frame()? {
            return Ok(frame);
        }
        Ok(self.static_provider.current_frame())
    }

    fn try_shared_frame(&self) -> Result<Option<ProvidedFrame>> {
        {
            let mut provider = self
                .shared_provider
                .lock()
                .expect("shared provider state poisoned");
            if let Some(active_provider) = provider.as_ref() {
                let frame = active_provider.try_current_frame()?;
                if !active_provider.is_active() {
                    *provider = None;
                }
                if frame.is_some() {
                    return Ok(frame);
                }
            }
        }

        let Some(provider) = SharedMemoryFeedProvider::open_matching(self.stream_format)? else {
            return Ok(None);
        };
        let frame = provider.try_current_frame()?;
        *self
            .shared_provider
            .lock()
            .expect("shared provider state poisoned") = Some(provider);
        Ok(frame)
    }
}

#[derive(Clone)]
struct CachedNv12Frame {
    frame_id: u64,
    bytes: Arc<[u8]>,
}

pub struct StreamShared {
    source_ref: SourceReference,
    self_iface: Mutex<Option<IMFMediaStream>>,
    self_iface2: Mutex<Option<IMFMediaStream2>>,
    event_queue: windows::Win32::Media::MediaFoundation::IMFMediaEventQueue,
    descriptor: IMFStreamDescriptor,
    attributes: IMFAttributes,
    format: VideoFormat,
    providers: FrameProviders,
    state: Mutex<StreamState>,
    selected_media_type: Mutex<Option<IMFMediaType>>,
    sample_allocator: Mutex<SampleAllocatorState>,
    nv12_cache: Mutex<Option<CachedNv12Frame>>,
}

impl StreamShared {
    pub(crate) fn create(
        source_ref: SourceReference,
        format: VideoFormat,
        shared_provider: Option<SharedMemoryFeedProvider>,
    ) -> Result<Arc<Self>> {
        debug_log("StreamShared::create");
        let attributes = create_attributes(10)?;
        let event_queue = unsafe { MFCreateEventQueue()? };
        let rgb32 = create_media_type(format, MFVideoFormat_RGB32)?;
        let nv12 = create_media_type(format, MFVideoFormat_NV12)?;
        let media_types = [Some(rgb32.clone()), Some(nv12.clone())];
        let descriptor = unsafe { MFCreateStreamDescriptor(STREAM_ID, &media_types)? };
        let handler: IMFMediaTypeHandler = unsafe { descriptor.GetMediaTypeHandler()? };
        let descriptor_attributes: IMFAttributes = descriptor.cast()?;
        unsafe {
            handler.SetCurrentMediaType(&nv12)?;
            attributes.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
            attributes.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, STREAM_ID)?;
            attributes.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
            attributes.SetUINT32(
                &MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES,
                MFFrameSourceTypes_Color.0 as u32,
            )?;
            descriptor_attributes.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
            descriptor_attributes.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, STREAM_ID)?;
            descriptor_attributes.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
            descriptor_attributes.SetUINT32(
                &MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES,
                MFFrameSourceTypes_Color.0 as u32,
            )?;
        }

        Ok(Arc::new(Self {
            source_ref,
            self_iface: Mutex::new(None),
            self_iface2: Mutex::new(None),
            event_queue,
            descriptor,
            attributes,
            format,
            providers: FrameProviders::new(format, shared_provider),
            state: Mutex::new(StreamState {
                stream_state: MF_STREAM_STATE_STOPPED,
                shutdown: false,
            }),
            selected_media_type: Mutex::new(None),
            sample_allocator: Mutex::new(SampleAllocatorState { allocator: None }),
            nv12_cache: Mutex::new(None),
        }))
    }

    pub fn bind(&self, stream: IMFMediaStream) {
        *self.self_iface.lock().expect("stream reference poisoned") = Some(stream);
    }

    pub fn bind2(&self, stream: IMFMediaStream2) {
        *self.self_iface2.lock().expect("stream2 reference poisoned") = Some(stream);
    }

    pub fn interface2(&self) -> Result<IMFMediaStream2> {
        self.self_iface2
            .lock()
            .expect("stream2 reference poisoned")
            .clone()
            .ok_or_else(|| E_POINTER.into())
    }

    pub fn descriptor(&self) -> IMFStreamDescriptor {
        self.descriptor.clone()
    }

    pub fn attributes(&self) -> IMFAttributes {
        self.attributes.clone()
    }

    pub fn set_stream_state(&self, state: MF_STREAM_STATE) -> Result<()> {
        self.ensure_active()?;
        self.state.lock().expect("stream state poisoned").stream_state = state;
        Ok(())
    }

    pub fn current_stream_state(&self) -> Result<MF_STREAM_STATE> {
        self.ensure_active()?;
        Ok(self.state.lock().expect("stream state poisoned").stream_state)
    }

    pub fn start(&self, start_position: &PROPVARIANT) -> Result<()> {
        debug_log("StreamShared::start");
        self.ensure_active()?;
        self.state.lock().expect("stream state poisoned").stream_state = MF_STREAM_STATE_RUNNING;
        queue_var_event(&self.event_queue, MEStreamStarted.0 as u32, start_position)
    }

    pub fn stop(&self) -> Result<()> {
        self.ensure_active()?;
        self.state.lock().expect("stream state poisoned").stream_state = MF_STREAM_STATE_STOPPED;
        let event_value = PROPVARIANT::new();
        queue_var_event(&self.event_queue, MEStreamStopped.0 as u32, &event_value)
    }

    pub fn shutdown(&self) -> Result<()> {
        let mut state = self.state.lock().expect("stream state poisoned");
        if state.shutdown {
            return Ok(());
        }
        state.shutdown = true;
        drop(state);
        self.sample_allocator
            .lock()
            .expect("sample allocator state poisoned")
            .allocator = None;
        unsafe { self.event_queue.Shutdown() }
    }

    fn ensure_active(&self) -> Result<()> {
        if self.state.lock().expect("stream state poisoned").shutdown {
            return Err(MF_E_SHUTDOWN.into());
        }
        Ok(())
    }

    fn current_media_type(&self) -> Result<IMFMediaType> {
        if let Some(media_type) = self
            .selected_media_type
            .lock()
            .expect("selected media type poisoned")
            .clone()
        {
            return Ok(media_type);
        }

        let handler = unsafe { self.descriptor.GetMediaTypeHandler()? };
        unsafe { handler.GetCurrentMediaType() }
    }

    pub fn set_current_media_type_override(&self, media_type: IMFMediaType) -> Result<()> {
        self.ensure_active()?;
        *self
            .selected_media_type
            .lock()
            .expect("selected media type poisoned") = Some(media_type);
        Ok(())
    }

    pub fn set_sample_allocator(&self, allocator: Option<IMFVideoSampleAllocator>) -> Result<()> {
        debug_log("StreamShared::set_sample_allocator");
        self.ensure_active()?;
        self.sample_allocator
            .lock()
            .expect("sample allocator state poisoned")
            .allocator = allocator;
        Ok(())
    }

    pub fn request_sample(&self, token: Option<&IUnknown>) -> Result<()> {
        debug_log("StreamShared::request_sample");
        self.ensure_active()?;
        if self.current_stream_state()? != MF_STREAM_STATE_RUNNING {
            return Err(MF_E_INVALIDREQUEST.into());
        }

        let media_type = self.current_media_type()?;
        let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE)? };
        let frame = self.providers.current_frame()?;
        let sample_time_100ns = frame
            .sample_time_100ns
            .unwrap_or_else(|| unsafe { MFGetSystemTime() });

        let frame_bytes: Arc<[u8]> = if subtype == MFVideoFormat_NV12 {
            self.current_nv12_frame(&frame)?
        } else {
            frame.bgra.clone()
        };

        let sample = create_memory_backed_sample(frame_bytes.len() as u32)?;
        let buffer: IMFMediaBuffer = unsafe { sample.GetBufferByIndex(0)? };
        unsafe {
            self.write_frame_to_buffer(&buffer, subtype, frame_bytes.as_ref())?;
            buffer.SetCurrentLength(frame_bytes.len() as u32)?;
            sample.SetSampleTime(sample_time_100ns)?;
            sample.SetSampleDuration(self.format.frame_duration_100ns())?;
            if let Some(token) = token {
                sample.SetUnknown(&MFSampleExtension_Token, token)?;
            }
            let sample_unknown: IUnknown = sample.cast()?;
            queue_unknown_event(&self.event_queue, MEMediaSample.0 as u32, &sample_unknown)?;
        }
        Ok(())
    }

    fn current_nv12_frame(&self, frame: &ProvidedFrame) -> Result<Arc<[u8]>> {
        if let Some(nv12) = &frame.nv12 {
            return Ok(nv12.clone());
        }

        let mut cache = self.nv12_cache.lock().expect("NV12 cache poisoned");
        if let Some(cached) = &*cache {
            if cached.frame_id == frame.frame_id {
                return Ok(cached.bytes.clone());
            }
        }

        let converted = Arc::<[u8]>::from(bgra_to_nv12_bytes(self.format, frame.bgra.as_ref())?);
        *cache = Some(CachedNv12Frame {
            frame_id: frame.frame_id,
            bytes: converted.clone(),
        });
        Ok(converted)
    }

    fn write_frame_to_buffer(
        &self,
        buffer: &IMFMediaBuffer,
        subtype: GUID,
        frame_bytes: &[u8],
    ) -> Result<()> {
        if let Ok(buffer2d) = buffer.cast::<IMF2DBuffer2>() {
            unsafe {
                let mut scanline0 = std::ptr::null_mut();
                let mut pitch = 0i32;
                let mut buffer_start = std::ptr::null_mut();
                let mut buffer_len = 0u32;
                buffer2d.Lock2DSize(
                    MF2DBuffer_LockFlags_Write,
                    &mut scanline0,
                    &mut pitch,
                    &mut buffer_start,
                    &mut buffer_len,
                )?;
                let write_result = if subtype == MFVideoFormat_NV12 {
                    copy_nv12_to_surface(self.format, frame_bytes, scanline0, pitch, buffer_start, buffer_len)
                } else {
                    copy_bgra_to_surface(self.format, frame_bytes, scanline0, pitch, buffer_len)
                };
                let unlock_result = buffer2d.Unlock2D();
                write_result?;
                unlock_result?;
                return Ok(());
            }
        }

        if let Ok(buffer2d) = buffer.cast::<IMF2DBuffer>() {
            unsafe {
                return buffer2d.ContiguousCopyFrom(frame_bytes);
            }
        }

        unsafe {
            let mut raw = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut current_len = 0u32;
            buffer.Lock(&mut raw, Some(&mut max_len), Some(&mut current_len))?;
            if max_len < frame_bytes.len() as u32 {
                let unlock_result = buffer.Unlock();
                unlock_result?;
                return Err(Error::from(E_POINTER));
            }
            std::ptr::copy_nonoverlapping(frame_bytes.as_ptr(), raw, frame_bytes.len());
            buffer.Unlock()
        }
    }

    #[allow(dead_code)]
    pub fn end_of_stream(&self) -> Result<()> {
        self.ensure_active()?;
        let event_value = PROPVARIANT::new();
        queue_var_event(&self.event_queue, MEEndOfPresentation.0 as u32, &event_value)
    }
}

#[implement(IMFMediaStream, IMFMediaStream2)]
pub struct StaticImageMediaStream {
    shared: Arc<StreamShared>,
}

impl StaticImageMediaStream {
    pub(crate) fn create(
        source_ref: SourceReference,
        format: VideoFormat,
        shared_provider: Option<SharedMemoryFeedProvider>,
    ) -> Result<(Arc<StreamShared>, IMFMediaStream)> {
        let shared = StreamShared::create(source_ref, format, shared_provider)?;
        let stream: IMFMediaStream = Self {
            shared: shared.clone(),
        }
        .into();
        let stream2: IMFMediaStream2 = stream.cast()?;
        shared.bind(stream.clone());
        shared.bind2(stream2);
        Ok((shared, stream))
    }
}

impl IMFMediaEventGenerator_Impl for StaticImageMediaStream_Impl {
    fn GetEvent(&self, dwflags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        unsafe { self.shared.event_queue.GetEvent(dwflags.0 as u32) }
    }

    fn BeginGetEvent(
        &self,
        pcallback: Option<&IMFAsyncCallback>,
        punkstate: Option<&IUnknown>,
    ) -> Result<()> {
        unsafe { self.shared.event_queue.BeginGetEvent(pcallback, punkstate) }
    }

    fn EndGetEvent(&self, presult: Option<&IMFAsyncResult>) -> Result<IMFMediaEvent> {
        unsafe { self.shared.event_queue.EndGetEvent(presult) }
    }

    fn QueueEvent(
        &self,
        met: u32,
        guidextendedtype: *const GUID,
        hrstatus: windows::core::HRESULT,
        pvvalue: *const PROPVARIANT,
    ) -> Result<()> {
        unsafe {
            self.shared
                .event_queue
                .QueueEventParamVar(met, guidextendedtype, hrstatus, pvvalue)
        }
    }
}

impl IMFMediaStream_Impl for StaticImageMediaStream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        self.shared.source_ref.get()
    }

    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        Ok(self.shared.descriptor())
    }

    fn RequestSample(&self, ptoken: Option<&IUnknown>) -> Result<()> {
        debug_log("IMFMediaStream::RequestSample");
        self.shared.request_sample(ptoken)
    }
}

impl IMFMediaStream2_Impl for StaticImageMediaStream_Impl {
    fn SetStreamState(&self, value: MF_STREAM_STATE) -> Result<()> {
        self.shared.set_stream_state(value)
    }

    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        self.shared.current_stream_state()
    }
}

fn create_attributes(initial_size: u32) -> Result<IMFAttributes> {
    let mut attributes = None;
    unsafe {
        MFCreateAttributes(&mut attributes, initial_size)?;
    }
    attributes.ok_or_else(|| Error::from(E_POINTER))
}

fn create_media_type(format: VideoFormat, subtype: GUID) -> Result<IMFMediaType> {
    let sample_size = if subtype == MFVideoFormat_NV12 {
        format.nv12_frame_bytes() as u32
    } else {
        format.bgra_frame_bytes() as u32
    };
    let stride = if subtype == MFVideoFormat_NV12 {
        format.nv12_stride() as u32
    } else {
        format.bgra_stride() as u32
    };

    let media_type = unsafe { MFCreateMediaType()? };
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack_u32_pair(format.width, format.height))?;
        media_type.SetUINT64(&MF_MT_FRAME_RATE, pack_u32_pair(format.fps_num, format.fps_den))?;
        media_type.SetUINT64(
            &MF_MT_FRAME_RATE_RANGE_MIN,
            pack_u32_pair(format.fps_num, format.fps_den),
        )?;
        media_type.SetUINT64(
            &MF_MT_FRAME_RATE_RANGE_MAX,
            pack_u32_pair(format.fps_num, format.fps_den),
        )?;
        media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_u32_pair(1, 1))?;
        media_type.SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)?;
        media_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        media_type.SetUINT32(&MF_MT_SAMPLE_SIZE, sample_size)?;
        media_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, stride)?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media_type.SetUINT32(
            &MF_MT_AVG_BITRATE,
            sample_size.saturating_mul(format.fps_num).saturating_mul(8),
        )?;
    }
    Ok(media_type)
}

fn pack_u32_pair(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

fn create_memory_backed_sample(buffer_size: u32) -> Result<IMFSample> {
    let sample = unsafe { MFCreateSample()? };
    let buffer = unsafe { MFCreateMemoryBuffer(buffer_size)? };
    unsafe {
        sample.AddBuffer(&buffer)?;
    }
    Ok(sample)
}

fn queue_var_event(
    queue: &windows::Win32::Media::MediaFoundation::IMFMediaEventQueue,
    event_type: u32,
    value: &PROPVARIANT,
) -> Result<()> {
    let event = CustomMediaEvent::from_propvariant(event_type, value.clone())?;
    unsafe { queue.QueueEvent(&event) }
}

fn queue_unknown_event(
    queue: &windows::Win32::Media::MediaFoundation::IMFMediaEventQueue,
    event_type: u32,
    value: &IUnknown,
) -> Result<()> {
    let event = CustomMediaEvent::from_unknown(event_type, value)?;
    unsafe { queue.QueueEvent(&event) }
}

pub fn try_open_shared_feed_provider() -> Result<Option<SharedMemoryFeedProvider>> {
    SharedMemoryFeedProvider::open_active()
}
