use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use windows::core::{implement, GUID, IUnknown, Interface, PCWSTR, Result};
use windows::Win32::Foundation::{ERROR_SET_NOT_FOUND, E_POINTER, S_OK};
use windows::Win32::Media::KernelStreaming::{IKsControl_Impl, KSIDENTIFIER};
use windows::Win32::Media::MediaFoundation::{
    IMFAsyncCallback, IMFAsyncResult, IMFAttributes, IMFGetService_Impl, IMFMediaEvent,
    IMFMediaEventGenerator_Impl, IMFMediaSource, IMFMediaSourceEx, IMFMediaSourceEx_Impl,
    IMFMediaSource_Impl, IMFPresentationDescriptor, IMFSampleAllocatorControl_Impl,
    IMFVideoSampleAllocator, MFCreateAttributes, MFCreateEventQueue, MFCreatePresentationDescriptor,
    MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MFMEDIASOURCE_IS_LIVE,
    MF_E_INVALIDREQUEST, MF_E_SHUTDOWN, MF_E_UNSUPPORTED_SERVICE,
    MF_E_UNSUPPORTED_TIME_FORMAT, MENewStream, MESourceStarted, MESourceStopped,
    MEUpdatedStream, MFSampleAllocatorUsage, MFSampleAllocatorUsage_UsesProvidedAllocator,
};
use windows_core::PROPVARIANT;

use crate::constants::{FRIENDLY_NAME, STREAM_ID};
use crate::media_stream::{SourceReference, StaticImageMediaStream, StreamShared};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceState {
    Stopped,
    Started,
    Shutdown,
}

pub struct SourceShared {
    event_queue: windows::Win32::Media::MediaFoundation::IMFMediaEventQueue,
    presentation_descriptor: IMFPresentationDescriptor,
    attributes: IMFAttributes,
    stream: Arc<StreamShared>,
    state: Mutex<SourceState>,
}

impl SourceShared {
    fn ensure_active(&self) -> Result<()> {
        if *self.state.lock().expect("source state poisoned") == SourceState::Shutdown {
            return Err(MF_E_SHUTDOWN.into());
        }
        Ok(())
    }

    fn create_presentation_descriptor(&self) -> Result<IMFPresentationDescriptor> {
        self.ensure_active()?;
        let descriptor = unsafe { self.presentation_descriptor.Clone()? };
        unsafe {
            descriptor.SelectStream(0)?;
        }
        Ok(descriptor)
    }

    fn start(&self, descriptor: Option<&IMFPresentationDescriptor>) -> Result<()> {
        self.ensure_active()?;
        if let Some(descriptor) = descriptor {
            unsafe {
                descriptor.SelectStream(0)?;
            }
        }

        let stream_unknown: IUnknown = self.stream.interface()?.cast()?;
        let event_type = if *self.state.lock().expect("source state poisoned") == SourceState::Started
        {
            MEUpdatedStream
        } else {
            MENewStream
        };

        self.stream.start()?;
        unsafe {
            self.event_queue.QueueEventParamUnk(
                event_type.0 as u32,
                std::ptr::null(),
                S_OK,
                &stream_unknown,
            )?;
        }
        *self.state.lock().expect("source state poisoned") = SourceState::Started;
        unsafe {
            self.event_queue.QueueEventParamVar(
                MESourceStarted.0 as u32,
                std::ptr::null(),
                S_OK,
                std::ptr::null(),
            )?;
        }
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        self.ensure_active()?;
        if *self.state.lock().expect("source state poisoned") == SourceState::Stopped {
            return Ok(());
        }
        self.stream.stop()?;
        *self.state.lock().expect("source state poisoned") = SourceState::Stopped;
        unsafe {
            self.event_queue
                .QueueEventParamVar(
                    MESourceStopped.0 as u32,
                    std::ptr::null(),
                    S_OK,
                    std::ptr::null(),
                )?;
        }
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        let mut state = self.state.lock().expect("source state poisoned");
        if *state == SourceState::Shutdown {
            return Ok(());
        }
        *state = SourceState::Shutdown;
        drop(state);
        self.stream.shutdown()?;
        unsafe { self.event_queue.Shutdown() }
    }
}

#[implement(IMFMediaSourceEx, windows::Win32::Media::MediaFoundation::IMFGetService, windows::Win32::Media::KernelStreaming::IKsControl, windows::Win32::Media::MediaFoundation::IMFSampleAllocatorControl)]
pub struct StaticImageMediaSource {
    shared: Arc<SourceShared>,
}

impl StaticImageMediaSource {
    pub fn create() -> Result<IMFMediaSourceEx> {
        let source_ref = SourceReference::new();
        let (stream_shared, _stream_iface) = StaticImageMediaStream::create(source_ref.clone())?;
        let stream_descriptors = [Some(stream_shared.descriptor())];
        let presentation_descriptor = unsafe { MFCreatePresentationDescriptor(Some(&stream_descriptors))? };
        let attributes = create_source_attributes()?;
        let event_queue = unsafe { MFCreateEventQueue()? };

        let source_ex: IMFMediaSourceEx = Self {
            shared: Arc::new(SourceShared {
                event_queue,
                presentation_descriptor,
                attributes,
                stream: stream_shared,
                state: Mutex::new(SourceState::Stopped),
            }),
        }
        .into();

        source_ref.bind(source_ex.cast::<IMFMediaSource>()?);
        Ok(source_ex)
    }
}

impl IMFMediaEventGenerator_Impl for StaticImageMediaSource_Impl {
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

impl IMFMediaSource_Impl for StaticImageMediaSource_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }

    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        self.shared.create_presentation_descriptor()
    }

    fn Start(
        &self,
        ppresentationdescriptor: Option<&IMFPresentationDescriptor>,
        pguidtimeformat: *const GUID,
        _pvarstartposition: *const PROPVARIANT,
    ) -> Result<()> {
        unsafe {
            if !pguidtimeformat.is_null() && *pguidtimeformat != GUID::zeroed() {
                return Err(MF_E_UNSUPPORTED_TIME_FORMAT.into());
            }
        }
        self.shared.start(ppresentationdescriptor)
    }

    fn Stop(&self) -> Result<()> {
        self.shared.stop()
    }

    fn Pause(&self) -> Result<()> {
        Err(MF_E_INVALIDREQUEST.into())
    }

    fn Shutdown(&self) -> Result<()> {
        self.shared.shutdown()
    }
}

impl IMFMediaSourceEx_Impl for StaticImageMediaSource_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        Ok(self.shared.attributes.clone())
    }

    fn GetStreamAttributes(&self, dwstreamidentifier: u32) -> Result<IMFAttributes> {
        if dwstreamidentifier != STREAM_ID {
            return Err(MF_E_INVALIDREQUEST.into());
        }
        Ok(self.shared.stream.attributes())
    }

    fn SetD3DManager(&self, _pmanager: Option<&IUnknown>) -> Result<()> {
        Ok(())
    }
}

impl IMFGetService_Impl for StaticImageMediaSource_Impl {
    fn GetService(
        &self,
        _guidservice: *const GUID,
        _riid: *const GUID,
        _ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

impl IKsControl_Impl for StaticImageMediaSource_Impl {
    fn KsProperty(
        &self,
        _property: *const KSIDENTIFIER,
        _propertylength: u32,
        _propertydata: *mut c_void,
        _datalength: u32,
        bytesreturned: *mut u32,
    ) -> Result<()> {
        unsafe {
            if !bytesreturned.is_null() {
                *bytesreturned = 0;
            }
        }
        Err(ERROR_SET_NOT_FOUND.to_hresult().into())
    }

    fn KsMethod(
        &self,
        _method: *const KSIDENTIFIER,
        _methodlength: u32,
        _methoddata: *mut c_void,
        _datalength: u32,
        bytesreturned: *mut u32,
    ) -> Result<()> {
        unsafe {
            if !bytesreturned.is_null() {
                *bytesreturned = 0;
            }
        }
        Err(ERROR_SET_NOT_FOUND.to_hresult().into())
    }

    fn KsEvent(
        &self,
        _event: *const KSIDENTIFIER,
        _eventlength: u32,
        _eventdata: *mut c_void,
        _datalength: u32,
        bytesreturned: *mut u32,
    ) -> Result<()> {
        unsafe {
            if !bytesreturned.is_null() {
                *bytesreturned = 0;
            }
        }
        Err(ERROR_SET_NOT_FOUND.to_hresult().into())
    }
}

impl IMFSampleAllocatorControl_Impl for StaticImageMediaSource_Impl {
    fn SetDefaultAllocator(&self, dwoutputstreamid: u32, pallocator: Option<&IUnknown>) -> Result<()> {
        if dwoutputstreamid != STREAM_ID {
            return Err(MF_E_INVALIDREQUEST.into());
        }

        let allocator = pallocator
            .map(|unknown| unknown.cast::<IMFVideoSampleAllocator>())
            .transpose()?;
        self.shared.stream.set_sample_allocator(allocator)
    }

    fn GetAllocatorUsage(
        &self,
        dwoutputstreamid: u32,
        pdwinputstreamid: *mut u32,
        peusage: *mut MFSampleAllocatorUsage,
    ) -> Result<()> {
        unsafe {
            if pdwinputstreamid.is_null() || peusage.is_null() {
                return Err(E_POINTER.into());
            }
            *pdwinputstreamid = dwoutputstreamid;
            *peusage = MFSampleAllocatorUsage_UsesProvidedAllocator;
        }
        Ok(())
    }
}

fn create_attributes(initial_size: u32) -> Result<IMFAttributes> {
    let mut attributes = None;
    unsafe {
        MFCreateAttributes(&mut attributes, initial_size)?;
    }
    attributes.ok_or_else(|| E_POINTER.into())
}

fn create_source_attributes() -> Result<IMFAttributes> {
    let attributes = create_attributes(8)?;
    unsafe {
        attributes.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )?;
        let friendly_name = wide_null(FRIENDLY_NAME);
        attributes.SetString(
            &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
            PCWSTR(friendly_name.as_ptr()),
        )?;
        let symbolic_link = wide_null("rust-staticcam://prototype");
        attributes.SetString(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
            PCWSTR(symbolic_link.as_ptr()),
        )?;
    }
    Ok(attributes)
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
