use std::sync::{Arc, Mutex};

use windows::core::{implement, Error, GUID, IUnknown, Interface, Result};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Media::KernelStreaming::PINNAME_VIDEO_CAPTURE;
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMF2DBuffer2, IMFAsyncCallback, IMFAsyncResult, IMFAttributes, IMFMediaBuffer,
    IMFMediaEvent,
    IMFMediaEventGenerator_Impl, IMFMediaSource, IMFMediaStream, IMFMediaStream2,
    IMFMediaStream2_Impl, IMFMediaStream_Impl, IMFMediaType, IMFMediaTypeHandler, IMFSample,
    IMFStreamDescriptor, IMFVideoSampleAllocator, MF2DBuffer_LockFlags_Write, MFCreateAttributes,
    MFCreateEventQueue, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateStreamDescriptor, MFGetSystemTime, MFMediaType_Video, MFVideoFormat_NV12,
    MFVideoFormat_RGB32,
    MFVideoInterlace_Progressive, MFSampleExtension_Token, MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS,
    MEEndOfPresentation, MEMediaSample, MEStreamStarted, MEStreamStopped, MFFrameSourceTypes_Color,
    MF_E_INVALIDREQUEST, MF_E_SHUTDOWN, MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FIXED_SIZE_SAMPLES, MF_MT_FRAME_RATE,
    MF_MT_FRAME_RATE_RANGE_MAX, MF_MT_FRAME_RATE_RANGE_MIN, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SAMPLE_SIZE,
    MF_MT_SUBTYPE, MF_STREAM_STATE, MF_STREAM_STATE_RUNNING, MF_STREAM_STATE_STOPPED,
    MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MF_DEVICESTREAM_FRAMESERVER_SHARED,
    MF_DEVICESTREAM_STREAM_CATEGORY, MF_DEVICESTREAM_STREAM_ID,
};
use windows_core::PROPVARIANT;

use crate::constants::{
    BGRA_FRAME_BYTES, FRAME_DURATION_100NS, FRAME_HEIGHT, FRAME_RATE_DENOMINATOR,
    FRAME_RATE_NUMERATOR, FRAME_WIDTH, NV12_FRAME_BYTES, STREAM_ID,
};
use crate::debug_log;
use crate::media_event::CustomMediaEvent;
use crate::test_pattern::StaticTestPattern;

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

pub struct StreamShared {
    source_ref: SourceReference,
    self_iface: Mutex<Option<IMFMediaStream>>,
    self_iface2: Mutex<Option<IMFMediaStream2>>,
    event_queue: windows::Win32::Media::MediaFoundation::IMFMediaEventQueue,
    descriptor: IMFStreamDescriptor,
    attributes: IMFAttributes,
    pattern: StaticTestPattern,
    state: Mutex<StreamState>,
    selected_media_type: Mutex<Option<IMFMediaType>>,
    sample_allocator: Mutex<SampleAllocatorState>,
}

impl StreamShared {
    pub fn create(source_ref: SourceReference) -> Result<Arc<Self>> {
        debug_log("StreamShared::create");
        let attributes = create_attributes(10)?;
        let event_queue = unsafe { MFCreateEventQueue()? };
        let rgb32 = create_media_type(MFVideoFormat_RGB32, BGRA_FRAME_BYTES as u32, FRAME_WIDTH * 4)?;
        let nv12 = create_media_type(MFVideoFormat_NV12, NV12_FRAME_BYTES as u32, FRAME_WIDTH)?;
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
            pattern: StaticTestPattern::new(),
            state: Mutex::new(StreamState {
                stream_state: MF_STREAM_STATE_STOPPED,
                shutdown: false,
            }),
            selected_media_type: Mutex::new(None),
            sample_allocator: Mutex::new(SampleAllocatorState {
                allocator: None,
            }),
        }))
    }

    pub fn bind(&self, stream: IMFMediaStream) {
        *self.self_iface.lock().expect("stream reference poisoned") = Some(stream);
    }

    pub fn bind2(&self, stream: IMFMediaStream2) {
        *self.self_iface2.lock().expect("stream2 reference poisoned") = Some(stream);
    }

    pub fn interface(&self) -> Result<IMFMediaStream> {
        self.self_iface
            .lock()
            .expect("stream reference poisoned")
            .clone()
            .ok_or_else(|| E_POINTER.into())
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
        debug_log("StreamShared::start enter");
        self.ensure_active()?;
        debug_log("StreamShared::start after ensure_active");
        self.state.lock().expect("stream state poisoned").stream_state = MF_STREAM_STATE_RUNNING;
        debug_log("StreamShared::start after state set");
        debug_log("StreamShared::start before queue_var_event");
        queue_var_event(&self.event_queue, MEStreamStarted.0 as u32, start_position)?;
        debug_log("StreamShared::start after queue_var_event");
        debug_log("StreamShared::start exit");
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        self.ensure_active()?;
        self.state.lock().expect("stream state poisoned").stream_state = MF_STREAM_STATE_STOPPED;
        let event_value = PROPVARIANT::new();
        queue_var_event(&self.event_queue, MEStreamStopped.0 as u32, &event_value)?;
        Ok(())
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
        let mut state = self
            .sample_allocator
            .lock()
            .expect("sample allocator state poisoned");
        state.allocator = allocator;
        Ok(())
    }

    pub fn request_sample(&self, token: Option<&IUnknown>) -> Result<()> {
        debug_log("StreamShared::request_sample enter");
        self.ensure_active()?;
        if self.current_stream_state()? != MF_STREAM_STATE_RUNNING {
            return Err(MF_E_INVALIDREQUEST.into());
        }

        let media_type = self.current_media_type()?;
        let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE)? };
        let frame_bytes = if subtype == MFVideoFormat_NV12 {
            self.pattern.nv12_bytes()
        } else {
            self.pattern.rgb32_bytes()
        };

        let sample = create_memory_backed_sample(frame_bytes.len() as u32)?;
        debug_log("StreamShared::request_sample allocated sample");
        let buffer: IMFMediaBuffer = unsafe { sample.GetBufferByIndex(0)? };
        debug_log("StreamShared::request_sample got buffer");
        unsafe {
            self.write_frame_to_buffer(&buffer, subtype, frame_bytes)?;
            debug_log("StreamShared::request_sample wrote buffer");
            buffer.SetCurrentLength(frame_bytes.len() as u32)?;
            sample.SetSampleTime(MFGetSystemTime())?;
            sample.SetSampleDuration(FRAME_DURATION_100NS)?;
            if let Some(token) = token {
                sample.SetUnknown(&MFSampleExtension_Token, token)?;
            }
            let sample_unknown: IUnknown = sample.cast()?;
            queue_unknown_event(&self.event_queue, MEMediaSample.0 as u32, &sample_unknown)?;
        }
        debug_log("StreamShared::request_sample exit");
        Ok(())
    }

    fn write_frame_to_buffer(
        &self,
        buffer: &IMFMediaBuffer,
        subtype: GUID,
        frame_bytes: &[u8],
    ) -> Result<()> {
        debug_log("StreamShared::write_frame_to_buffer");
        if let Ok(buffer2d) = buffer.cast::<IMF2DBuffer2>() {
            debug_log("StreamShared::write_frame_to_buffer 2d");
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
                    self.pattern
                        .copy_to_nv12_surface(scanline0, pitch, buffer_start, buffer_len)
                } else {
                    self.pattern
                        .copy_to_rgb32_surface(scanline0, pitch, buffer_len)
                };
                let unlock_result = buffer2d.Unlock2D();
                write_result?;
                unlock_result?;
                return Ok(());
            }
        }

        if let Ok(buffer2d) = buffer.cast::<IMF2DBuffer>() {
            debug_log("StreamShared::write_frame_to_buffer contiguous 2d");
            unsafe {
                return if subtype == MFVideoFormat_NV12 {
                    buffer2d.ContiguousCopyFrom(self.pattern.nv12_bytes())
                } else {
                    buffer2d.ContiguousCopyFrom(self.pattern.rgb32_bytes())
                };
            }
        }

        debug_log("StreamShared::write_frame_to_buffer lock copy");
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
        queue_var_event(&self.event_queue, MEEndOfPresentation.0 as u32, &event_value)?;
        Ok(())
    }
}

#[implement(IMFMediaStream, IMFMediaStream2)]
pub struct StaticImageMediaStream {
    shared: Arc<StreamShared>,
}

impl StaticImageMediaStream {
    pub fn create(source_ref: SourceReference) -> Result<(Arc<StreamShared>, IMFMediaStream)> {
        let shared = StreamShared::create(source_ref)?;
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

fn create_media_type(subtype: GUID, sample_size: u32, stride: u32) -> Result<IMFMediaType> {
    let media_type = unsafe { MFCreateMediaType()? };
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack_u32_pair(FRAME_WIDTH, FRAME_HEIGHT))?;
        media_type.SetUINT64(
            &MF_MT_FRAME_RATE,
            pack_u32_pair(FRAME_RATE_NUMERATOR, FRAME_RATE_DENOMINATOR),
        )?;
        media_type.SetUINT64(
            &MF_MT_FRAME_RATE_RANGE_MIN,
            pack_u32_pair(FRAME_RATE_NUMERATOR, FRAME_RATE_DENOMINATOR),
        )?;
        media_type.SetUINT64(
            &MF_MT_FRAME_RATE_RANGE_MAX,
            pack_u32_pair(FRAME_RATE_NUMERATOR, FRAME_RATE_DENOMINATOR),
        )?;
        media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_u32_pair(1, 1))?;
        media_type.SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)?;
        media_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        media_type.SetUINT32(&MF_MT_SAMPLE_SIZE, sample_size)?;
        media_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, stride)?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media_type.SetUINT32(
            &MF_MT_AVG_BITRATE,
            sample_size.saturating_mul(FRAME_RATE_NUMERATOR).saturating_mul(8),
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
