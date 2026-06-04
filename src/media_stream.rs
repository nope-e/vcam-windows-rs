use std::sync::{Arc, Mutex};

use windows::core::{implement, Error, GUID, IUnknown, Interface, Result};
use windows::Win32::Foundation::{E_POINTER, S_OK};
use windows::Win32::Media::KernelStreaming::PINNAME_VIDEO_CAPTURE;
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMF2DBuffer2, IMFAsyncCallback, IMFAsyncResult, IMFAttributes, IMFMediaBuffer,
    IMFMediaEvent,
    IMFMediaEventGenerator_Impl, IMFMediaSource, IMFMediaStream, IMFMediaStream2,
    IMFMediaStream2_Impl, IMFMediaStream_Impl, IMFMediaType, IMFMediaTypeHandler, IMFSample,
    IMFStreamDescriptor, IMFVideoSampleAllocator, MF2DBuffer_LockFlags_Write, MFCreateAttributes,
    MFCreateEventQueue, MFCreateMediaType, MFCreateStreamDescriptor, MFCreateVideoSampleAllocatorEx,
    MFGetSystemTime, MFMediaType_Video, MFVideoFormat_NV12, MFVideoFormat_RGB32,
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
use windows_core::{PROPVARIANT, Type};

use crate::constants::{
    BGRA_FRAME_BYTES, FRAME_DURATION_100NS, FRAME_HEIGHT, FRAME_RATE_DENOMINATOR,
    FRAME_RATE_NUMERATOR, FRAME_WIDTH, NV12_FRAME_BYTES, STREAM_ID,
};
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
    initialized_subtype: Option<GUID>,
}

pub struct StreamShared {
    source_ref: SourceReference,
    self_iface: Mutex<Option<IMFMediaStream>>,
    event_queue: windows::Win32::Media::MediaFoundation::IMFMediaEventQueue,
    descriptor: IMFStreamDescriptor,
    attributes: IMFAttributes,
    pattern: StaticTestPattern,
    state: Mutex<StreamState>,
    sample_allocator: Mutex<SampleAllocatorState>,
}

impl StreamShared {
    pub fn create(source_ref: SourceReference) -> Result<Arc<Self>> {
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
            event_queue,
            descriptor,
            attributes,
            pattern: StaticTestPattern::new(),
            state: Mutex::new(StreamState {
                stream_state: MF_STREAM_STATE_STOPPED,
                shutdown: false,
            }),
            sample_allocator: Mutex::new(SampleAllocatorState {
                allocator: None,
                initialized_subtype: None,
            }),
        }))
    }

    pub fn bind(&self, stream: IMFMediaStream) {
        *self.self_iface.lock().expect("stream reference poisoned") = Some(stream);
    }

    pub fn interface(&self) -> Result<IMFMediaStream> {
        self.self_iface
            .lock()
            .expect("stream reference poisoned")
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

    pub fn start(&self) -> Result<()> {
        self.ensure_active()?;
        let media_type = self.current_media_type()?;
        let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE)? };
        self.ensure_sample_allocator(&media_type, subtype)?;
        self.state.lock().expect("stream state poisoned").stream_state = MF_STREAM_STATE_RUNNING;
        unsafe {
            self.event_queue
                .QueueEventParamVar(
                    MEStreamStarted.0 as u32,
                    std::ptr::null(),
                    S_OK,
                    std::ptr::null(),
                )?;
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        self.ensure_active()?;
        self.state.lock().expect("stream state poisoned").stream_state = MF_STREAM_STATE_STOPPED;
        unsafe {
            self.event_queue
                .QueueEventParamVar(
                    MEStreamStopped.0 as u32,
                    std::ptr::null(),
                    S_OK,
                    std::ptr::null(),
                )?;
        }
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
        let handler = unsafe { self.descriptor.GetMediaTypeHandler()? };
        unsafe { handler.GetCurrentMediaType() }
    }

    pub fn set_sample_allocator(&self, allocator: Option<IMFVideoSampleAllocator>) -> Result<()> {
        self.ensure_active()?;
        let mut state = self
            .sample_allocator
            .lock()
            .expect("sample allocator state poisoned");
        state.allocator = allocator;
        state.initialized_subtype = None;
        Ok(())
    }

    pub fn request_sample(&self, token: Option<&IUnknown>) -> Result<()> {
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

        let allocator = self.ensure_sample_allocator(&media_type, subtype)?;
        let sample: IMFSample = unsafe { allocator.AllocateSample()? };
        let buffer: IMFMediaBuffer = unsafe { sample.GetBufferByIndex(0)? };
        unsafe {
            self.write_frame_to_buffer(&buffer, subtype)?;
            buffer.SetCurrentLength(frame_bytes.len() as u32)?;
            sample.SetSampleTime(MFGetSystemTime())?;
            sample.SetSampleDuration(FRAME_DURATION_100NS)?;
            if let Some(token) = token {
                sample.SetUnknown(&MFSampleExtension_Token, token)?;
            }
            let sample_unknown: IUnknown = sample.cast()?;
            self.event_queue.QueueEventParamUnk(
                MEMediaSample.0 as u32,
                std::ptr::null(),
                S_OK,
                &sample_unknown,
            )?;
        }
        Ok(())
    }

    fn ensure_sample_allocator(
        &self,
        media_type: &IMFMediaType,
        subtype: GUID,
    ) -> Result<IMFVideoSampleAllocator> {
        let mut allocator_state = self
            .sample_allocator
            .lock()
            .expect("sample allocator state poisoned");
        if allocator_state.allocator.is_none() {
            let allocator = create_video_sample_allocator()?;
            allocator_state.allocator = Some(allocator);
        }

        let allocator = allocator_state
            .allocator
            .clone()
            .ok_or_else(|| Error::from(E_POINTER))?;

        if allocator_state.initialized_subtype != Some(subtype) {
            unsafe {
                let _ = allocator.UninitializeSampleAllocator();
                allocator.InitializeSampleAllocator(3, media_type)?;
            }
            allocator_state.initialized_subtype = Some(subtype);
        }

        Ok(allocator)
    }

    fn write_frame_to_buffer(&self, buffer: &IMFMediaBuffer, subtype: GUID) -> Result<()> {
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

        let buffer2d: IMF2DBuffer = buffer.cast()?;
        unsafe {
            if subtype == MFVideoFormat_NV12 {
                buffer2d.ContiguousCopyFrom(self.pattern.nv12_bytes())
            } else {
                buffer2d.ContiguousCopyFrom(self.pattern.rgb32_bytes())
            }
        }
    }

    #[allow(dead_code)]
    pub fn end_of_stream(&self) -> Result<()> {
        self.ensure_active()?;
        unsafe {
            self.event_queue.QueueEventParamVar(
                MEEndOfPresentation.0 as u32,
                std::ptr::null(),
                S_OK,
                std::ptr::null(),
            )?;
        }
        Ok(())
    }
}

#[implement(IMFMediaStream2)]
pub struct StaticImageMediaStream {
    shared: Arc<StreamShared>,
}

impl StaticImageMediaStream {
    pub fn create(source_ref: SourceReference) -> Result<(Arc<StreamShared>, IMFMediaStream)> {
        let shared = StreamShared::create(source_ref)?;
        let stream2: IMFMediaStream2 = Self {
            shared: shared.clone(),
        }
        .into();
        let stream: IMFMediaStream = stream2.cast()?;
        shared.bind(stream.clone());
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

fn create_video_sample_allocator() -> Result<IMFVideoSampleAllocator> {
    let mut allocator = std::ptr::null_mut();
    unsafe {
        MFCreateVideoSampleAllocatorEx(&IMFVideoSampleAllocator::IID, &mut allocator)?;
        IMFVideoSampleAllocator::from_abi(allocator)
    }
}
