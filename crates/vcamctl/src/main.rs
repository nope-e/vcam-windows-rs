use std::env;
use std::ffi::CString;
use std::path::PathBuf;

use windows::core::{Error, GUID, HSTRING, HRESULT, IUnknown, Interface, PCSTR, PCWSTR, Result};
use windows::Win32::Foundation::{E_FAIL, FreeLibrary};
use windows::Win32::Media::KernelStreaming::{
    KSCATEGORY_CAPTURE, KSCATEGORY_VIDEO, KSCATEGORY_VIDEO_CAMERA,
};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaBuffer, IMFMediaSource, IMFMediaSourceEx, IMFMediaStream, IMFSample,
    IMFVirtualCamera, MFCreateVirtualCamera, MFShutdown, MFStartup, MFSTARTUP_FULL,
    MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime_System,
    MFVirtualCameraType_SoftwareCameraSource, MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS, MEMediaSample,
    MENewStream, MESourceStarted, MEStreamStarted, MEUpdatedStream, MF_API_VERSION, MF_MT_SUBTYPE,
    MF_SDK_VERSION, MFVideoFormat_NV12, MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use vcam_server::{
    debug_log, validate_dump_path, write_bgra_bmp, write_nv12_bmp, StaticTestPattern, ACTIVATE_CLSID,
    ACTIVATE_CLSID_STRING, FRIENDLY_NAME,
};
use vcam_server::registration::{self, RegistryScope};

type DllExport = unsafe extern "system" fn() -> HRESULT;

fn main() -> Result<()> {
    let _runtime = com::runtime::init_runtime()
        .map_err(|err| Error::new(E_FAIL.into(), format!("COM runtime init failed: {err:?}")))?;

    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("register-com") => {
            let options = parse_registration_options(&args[1..])?;
            if let Some(dll_path) = options.dll_path {
                registration::register_server(options.scope, Some(&dll_path))?;
            } else {
                match options.scope {
                    RegistryScope::CurrentUser => invoke_dll_export("DllRegisterServer", default_dll_path()?)?,
                    RegistryScope::LocalMachine => registration::register_server(
                        RegistryScope::LocalMachine,
                        Some(&default_dll_path()?),
                    )?,
                }
            }
        }
        Some("unregister-com") => {
            let options = parse_registration_options(&args[1..])?;
            match options.scope {
                RegistryScope::CurrentUser => invoke_dll_export("DllUnregisterServer", default_dll_path()?)?,
                RegistryScope::LocalMachine => registration::unregister_server(RegistryScope::LocalMachine)?,
            }
        }
        Some("create-camera") => {
            let camera = create_virtual_camera()
                .map_err(|err| Error::new(err.code(), format!("MFCreateVirtualCamera failed: {err}")))?;
            unsafe {
                camera
                    .Start(None)
                    .map_err(|err| Error::new(err.code(), format!("IMFVirtualCamera::Start failed: {err}")))?;
            }
            println!("Virtual camera started");
        }
        Some("remove-camera") => {
            let camera = create_virtual_camera()
                .map_err(|err| Error::new(err.code(), format!("MFCreateVirtualCamera failed: {err}")))?;
            unsafe {
                camera
                    .Remove()
                    .map_err(|err| Error::new(err.code(), format!("IMFVirtualCamera::Remove failed: {err}")))?;
            }
            println!("Virtual camera removed");
        }
        Some("probe-create") => {
            let _camera = create_virtual_camera()
                .map_err(|err| Error::new(err.code(), format!("MFCreateVirtualCamera failed: {err}")))?;
            println!("MFCreateVirtualCamera succeeded");
        }
        Some("dump-frame") => {
            let path = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("static-test-pattern.bmp"));
            validate_dump_path(&path)?;
            StaticTestPattern::new().write_bmp(&path)?;
            println!("wrote {}", path.display());
        }
        Some("dump-com-frame") => {
            let options = parse_dump_com_frame_options(&args[1..])?;
            validate_dump_path(&options.path)?;
            let report = dump_com_frame(&options)?;
            println!(
                "wrote {} via COM server ({}, {} bytes, content verified)",
                options.path.display(),
                report.subtype.label(),
                report.byte_len,
            );
        }
        _ => print_usage(),
    }

    Ok(())
}

struct RegistrationOptions {
    scope: RegistryScope,
    dll_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameSubtype {
    Rgb32,
    Nv12,
}

impl FrameSubtype {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "rgb32" => Ok(Self::Rgb32),
            "nv12" => Ok(Self::Nv12),
            _ => Err(Error::new(
                E_FAIL.into(),
                format!("invalid subtype '{value}', expected 'rgb32' or 'nv12'"),
            )),
        }
    }

    fn from_guid(value: GUID) -> Option<Self> {
        if value == MFVideoFormat_RGB32 {
            Some(Self::Rgb32)
        } else if value == MFVideoFormat_NV12 {
            Some(Self::Nv12)
        } else {
            None
        }
    }

    fn guid(self) -> GUID {
        match self {
            Self::Rgb32 => MFVideoFormat_RGB32,
            Self::Nv12 => MFVideoFormat_NV12,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Rgb32 => "rgb32",
            Self::Nv12 => "nv12",
        }
    }
}

struct DumpComFrameOptions {
    path: PathBuf,
    subtype: FrameSubtype,
}

struct DumpComFrameReport {
    subtype: FrameSubtype,
    byte_len: usize,
}

fn parse_registration_options(args: &[String]) -> Result<RegistrationOptions> {
    let mut scope = RegistryScope::CurrentUser;
    let mut dll_path = None;
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--scope" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| Error::new(E_FAIL.into(), "missing value for --scope"))?;
                scope = parse_scope(value)?;
                index += 2;
            }
            "--dll-path" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| Error::new(E_FAIL.into(), "missing value for --dll-path"))?;
                dll_path = Some(PathBuf::from(value));
                index += 2;
            }
            other => {
                return Err(Error::new(
                    E_FAIL.into(),
                    format!("unknown option for registration command: {other}"),
                ));
            }
        }
    }

    Ok(RegistrationOptions { scope, dll_path })
}

fn parse_scope(value: &str) -> Result<RegistryScope> {
    match value {
        "user" => Ok(RegistryScope::CurrentUser),
        "machine" => Ok(RegistryScope::LocalMachine),
        _ => Err(Error::new(
            E_FAIL.into(),
            format!("invalid scope '{value}', expected 'user' or 'machine'"),
        )),
    }
}

fn parse_dump_com_frame_options(args: &[String]) -> Result<DumpComFrameOptions> {
    let mut path = None;
    let mut subtype = FrameSubtype::Rgb32;
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--subtype" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| Error::new(E_FAIL.into(), "missing value for --subtype"))?;
                subtype = FrameSubtype::parse(value)?;
                index += 2;
            }
            option if option.starts_with("--") => {
                return Err(Error::new(
                    E_FAIL.into(),
                    format!("unknown option for dump-com-frame: {option}"),
                ));
            }
            value => {
                if path.is_some() {
                    return Err(Error::new(
                        E_FAIL.into(),
                        "dump-com-frame accepts at most one output path",
                    ));
                }
                path = Some(PathBuf::from(value));
                index += 1;
            }
        }
    }

    let default_name = match subtype {
        FrameSubtype::Rgb32 => "com-frame-rgb32.bmp",
        FrameSubtype::Nv12 => "com-frame-nv12.bmp",
    };
    Ok(DumpComFrameOptions {
        path: path.unwrap_or_else(|| PathBuf::from(default_name)),
        subtype,
    })
}

fn create_virtual_camera() -> Result<IMFVirtualCamera> {
    let version = ((MF_SDK_VERSION as u32) << 16) | MF_API_VERSION as u32;
    unsafe {
        MFStartup(version, MFSTARTUP_FULL)?;
    }
    let categories = [KSCATEGORY_VIDEO_CAMERA, KSCATEGORY_VIDEO, KSCATEGORY_CAPTURE];
    let camera = unsafe {
        MFCreateVirtualCamera(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_System,
            MFVirtualCameraAccess_CurrentUser,
            &HSTRING::from(FRIENDLY_NAME),
            &HSTRING::from(ACTIVATE_CLSID_STRING),
            Some(&categories),
        )?
    };
    Ok(camera)
}

fn dump_com_frame(options: &DumpComFrameOptions) -> Result<DumpComFrameReport> {
    debug_log("dump_com_frame enter");
    startup_media_foundation()?;
    debug_log("dump_com_frame after MFStartup");

    let mut activate: Option<IMFActivate> = None;
    let mut source: Option<IMFMediaSource> = None;
    let result = (|| -> Result<DumpComFrameReport> {
        debug_log("dump_com_frame before CoCreateInstance");
        let instance: IMFActivate = unsafe {
            CoCreateInstance(&ACTIVATE_CLSID, None::<&IUnknown>, CLSCTX_INPROC_SERVER)?
        };
        debug_log("dump_com_frame after CoCreateInstance");
        activate = Some(instance.clone());

        let source_ex: IMFMediaSourceEx = unsafe { instance.ActivateObject()? };
        debug_log("dump_com_frame after ActivateObject");
        let source_base: IMFMediaSource = source_ex.cast()?;
        source = Some(source_base.clone());

        let presentation_descriptor = unsafe { source_base.CreatePresentationDescriptor()? };
        debug_log("dump_com_frame after CreatePresentationDescriptor");
        select_stream_media_type(&presentation_descriptor, options.subtype)?;
        debug_log("dump_com_frame after select_stream_media_type");
        let start_position: windows::core::PROPVARIANT = 0i64.into();
        unsafe {
            source_base.Start(
                Some(&presentation_descriptor),
                std::ptr::null(),
                &start_position as *const _,
            )?;
        }
        debug_log("dump_com_frame after Start");

        let stream = wait_for_stream_from_source(&source_base)?;
        debug_log("dump_com_frame after wait_for_stream_from_source");
        wait_for_source_event(&source_base, MESourceStarted.0 as u32)?;
        debug_log("dump_com_frame after wait_for_source_event");
        wait_for_stream_event(&stream, MEStreamStarted.0 as u32)?;
        debug_log("dump_com_frame after wait_for_stream_event");
        unsafe {
            stream.RequestSample(None)?;
        }
        debug_log("dump_com_frame after RequestSample");

        let sample = wait_for_sample_from_stream(&stream)?;
        debug_log("dump_com_frame after wait_for_sample_from_stream");
        let actual_subtype = current_presentation_subtype(&presentation_descriptor)?;
        debug_log("dump_com_frame after current_presentation_subtype");
        let frame_bytes = read_sample_bytes(&sample)?;
        debug_log("dump_com_frame after read_sample_bytes");

        write_frame_dump(&options.path, actual_subtype, &frame_bytes)?;
        debug_log("dump_com_frame after write_frame_dump");
        verify_frame_payload(actual_subtype, &frame_bytes)?;
        debug_log("dump_com_frame after verify_frame_payload");

        Ok(DumpComFrameReport {
            subtype: actual_subtype,
            byte_len: frame_bytes.len(),
        })
    })();

    if let Some(source) = source {
        debug_log("dump_com_frame source shutdown");
        unsafe {
            let _ = source.Shutdown();
        }
    }
    if let Some(activate) = activate {
        debug_log("dump_com_frame activate shutdown");
        unsafe {
            let _ = activate.ShutdownObject();
        }
    }
    unsafe {
        let _ = MFShutdown();
    }
    debug_log("dump_com_frame exit");

    result
}

fn startup_media_foundation() -> Result<()> {
    let version = ((MF_SDK_VERSION as u32) << 16) | MF_API_VERSION as u32;
    unsafe { MFStartup(version, MFSTARTUP_FULL) }
}

fn current_presentation_subtype(
    presentation_descriptor: &windows::Win32::Media::MediaFoundation::IMFPresentationDescriptor,
) -> Result<FrameSubtype> {
    let stream_descriptor = get_stream_descriptor(presentation_descriptor)?;
    let handler = unsafe { stream_descriptor.GetMediaTypeHandler()? };
    let media_type = unsafe { handler.GetCurrentMediaType()? };
    let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE)? };
    FrameSubtype::from_guid(subtype).ok_or_else(|| {
        Error::new(
            E_FAIL.into(),
            format!("presentation descriptor returned unsupported subtype {subtype:?}"),
        )
    })
}

fn select_stream_media_type(
    presentation_descriptor: &windows::Win32::Media::MediaFoundation::IMFPresentationDescriptor,
    subtype: FrameSubtype,
) -> Result<()> {
    let stream_descriptor = get_stream_descriptor(presentation_descriptor)?;
    let handler = unsafe { stream_descriptor.GetMediaTypeHandler()? };
    let count = unsafe { handler.GetMediaTypeCount()? };

    for index in 0..count {
        let media_type = unsafe { handler.GetMediaTypeByIndex(index)? };
        let candidate = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE)? };
        if candidate == subtype.guid() {
            unsafe {
                handler.SetCurrentMediaType(&media_type)?;
            }
            return Ok(());
        }
    }

    Err(Error::new(
        E_FAIL.into(),
        format!("stream descriptor does not expose subtype {}", subtype.label()),
    ))
}

fn get_stream_descriptor(
    presentation_descriptor: &windows::Win32::Media::MediaFoundation::IMFPresentationDescriptor,
) -> Result<windows::Win32::Media::MediaFoundation::IMFStreamDescriptor> {
    let mut selected = windows::Win32::Foundation::BOOL(0);
    let mut descriptor = None;
    unsafe {
        presentation_descriptor.GetStreamDescriptorByIndex(0, &mut selected, &mut descriptor)?;
    }
    descriptor.ok_or_else(|| Error::from(E_FAIL))
}

fn wait_for_stream_from_source(source: &IMFMediaSource) -> Result<IMFMediaStream> {
    for _ in 0..4 {
        let event = unsafe { source.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))? };
        let event_type = unsafe { event.GetType()? };
        if event_type == MENewStream.0 as u32 || event_type == MEUpdatedStream.0 as u32 {
            let value = unsafe { event.GetValue()? };
            let unknown = IUnknown::try_from(&value)?;
            return unknown.cast();
        }
    }

    Err(Error::new(
        E_FAIL.into(),
        "source did not produce MENewStream/MEUpdatedStream",
    ))
}

fn wait_for_source_event(source: &IMFMediaSource, expected_event: u32) -> Result<()> {
    wait_for_event(
        || unsafe { source.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0)) },
        expected_event,
        "source",
    )
}

fn wait_for_stream_event(stream: &IMFMediaStream, expected_event: u32) -> Result<()> {
    wait_for_event(
        || unsafe { stream.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0)) },
        expected_event,
        "stream",
    )
}

fn wait_for_sample_from_stream(stream: &IMFMediaStream) -> Result<IMFSample> {
    for _ in 0..4 {
        let event = unsafe { stream.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))? };
        let event_type = unsafe { event.GetType()? };
        if event_type == MEMediaSample.0 as u32 {
            let value = unsafe { event.GetValue()? };
            let unknown = IUnknown::try_from(&value)?;
            return unknown.cast();
        }
    }

    Err(Error::new(
        E_FAIL.into(),
        "stream did not produce a MEMediaSample event",
    ))
}

fn wait_for_event<F>(mut next_event: F, expected_event: u32, label: &str) -> Result<()>
where
    F: FnMut() -> Result<windows::Win32::Media::MediaFoundation::IMFMediaEvent>,
{
    for _ in 0..4 {
        let event = next_event()?;
        let event_type = unsafe { event.GetType()? };
        if event_type == expected_event {
            return Ok(());
        }
    }

    Err(Error::new(
        E_FAIL.into(),
        format!("{label} did not produce event {expected_event}"),
    ))
}

fn read_sample_bytes(sample: &IMFSample) -> Result<Vec<u8>> {
    let buffer: IMFMediaBuffer = unsafe { sample.ConvertToContiguousBuffer()? };
    let byte_len = unsafe { buffer.GetCurrentLength()? } as usize;
    let mut data = vec![0u8; byte_len];

    unsafe {
        let mut raw = std::ptr::null_mut();
        let mut max_len = 0u32;
        let mut current_len = 0u32;
        buffer.Lock(&mut raw, Some(&mut max_len), Some(&mut current_len))?;
        std::ptr::copy_nonoverlapping(raw, data.as_mut_ptr(), current_len as usize);
        let unlock_result = buffer.Unlock();
        unlock_result?;
        data.truncate(current_len as usize);
    }

    Ok(data)
}

fn write_frame_dump(path: &PathBuf, subtype: FrameSubtype, frame_bytes: &[u8]) -> Result<()> {
    match subtype {
        FrameSubtype::Rgb32 => write_bgra_bmp(path, frame_bytes),
        FrameSubtype::Nv12 => write_nv12_bmp(path, frame_bytes),
    }
}

fn verify_frame_payload(subtype: FrameSubtype, frame_bytes: &[u8]) -> Result<()> {
    let expected_pattern = StaticTestPattern::new();
    let expected = match subtype {
        FrameSubtype::Rgb32 => expected_pattern.rgb32_bytes(),
        FrameSubtype::Nv12 => expected_pattern.nv12_bytes(),
    };

    if frame_bytes == expected {
        return Ok(());
    }

    Err(Error::new(
        E_FAIL.into(),
        format!("COM sample payload mismatch: {}", describe_mismatch(expected, frame_bytes)),
    ))
}

fn describe_mismatch(expected: &[u8], actual: &[u8]) -> String {
    if expected.len() != actual.len() {
        return format!(
            "expected {} bytes, got {} bytes",
            expected.len(),
            actual.len()
        );
    }

    if let Some(offset) = expected
        .iter()
        .zip(actual.iter())
        .position(|(left, right)| left != right)
    {
        return format!(
            "first differing byte at offset {offset}: expected {}, got {}",
            expected[offset],
            actual[offset],
        );
    }

    "unknown content mismatch".to_string()
}

fn default_dll_path() -> Result<PathBuf> {
    let exe = env::current_exe()
        .map_err(|err| Error::new(E_FAIL.into(), format!("current_exe failed: {err}")))?;
    let dll_name = "vcam_server.dll";
    Ok(exe
        .parent()
        .ok_or_else(|| Error::new(E_FAIL.into(), "executable path has no parent"))?
        .join(dll_name))
}

fn invoke_dll_export(export: &str, dll_path: PathBuf) -> Result<()> {
    let dll_path_w = wide_null(dll_path.as_os_str().to_string_lossy().as_ref());
    let export_c = CString::new(export)
        .map_err(|err| Error::new(E_FAIL.into(), format!("invalid export name: {err}")))?;

    unsafe {
        let module = LoadLibraryW(PCWSTR(dll_path_w.as_ptr()))?;

        let proc = GetProcAddress(module, PCSTR(export_c.as_ptr() as *const u8))
            .ok_or_else(Error::from_win32)?;
        let func: DllExport = std::mem::transmute(proc);
        let result = func();
        let free_result = FreeLibrary(module);
        result.ok()?;
        free_result?;
    }

    Ok(())
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn print_usage() {
    println!("Usage:");
    println!("  cargo run -p vcamctl -- register-com [--scope user|machine] [--dll-path <path>]");
    println!("  cargo run -p vcamctl -- unregister-com [--scope user|machine]");
    println!("  cargo run -p vcamctl -- create-camera");
    println!("  cargo run -p vcamctl -- remove-camera");
    println!("  cargo run -p vcamctl -- probe-create");
    println!("  cargo run -p vcamctl -- dump-frame [path]");
    println!("  cargo run -p vcamctl -- dump-com-frame [path] [--subtype rgb32|nv12]");
}
