use std::env;
use std::ffi::CString;
use std::path::PathBuf;

use windows::core::{Error, HSTRING, PCSTR, PCWSTR, Result, HRESULT};
use windows::Win32::Foundation::{E_FAIL, FreeLibrary};
use windows::Win32::Media::KernelStreaming::{
    KSCATEGORY_CAPTURE, KSCATEGORY_VIDEO, KSCATEGORY_VIDEO_CAMERA,
};
use windows::Win32::Media::MediaFoundation::{
    IMFVirtualCamera, MFCreateVirtualCamera, MFStartup, MFSTARTUP_FULL,
    MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime_System,
    MFVirtualCameraType_SoftwareCameraSource, MF_API_VERSION, MF_SDK_VERSION,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use vcam_windows_rs::{validate_dump_path, StaticTestPattern, ACTIVATE_CLSID_STRING, FRIENDLY_NAME};
use vcam_windows_rs::registration::{self, RegistryScope};

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
        _ => print_usage(),
    }

    Ok(())
}

struct RegistrationOptions {
    scope: RegistryScope,
    dll_path: Option<PathBuf>,
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

fn default_dll_path() -> Result<PathBuf> {
    let exe = env::current_exe()
        .map_err(|err| Error::new(E_FAIL.into(), format!("current_exe failed: {err}")))?;
    let dll_name = if cfg!(debug_assertions) {
        "vcam_windows_rs.dll"
    } else {
        "vcam_windows_rs.dll"
    };
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
    println!("  cargo run -- register-com [--scope user|machine] [--dll-path <path>]");
    println!("  cargo run -- unregister-com [--scope user|machine]");
    println!("  cargo run -- create-camera");
    println!("  cargo run -- remove-camera");
    println!("  cargo run -- probe-create");
    println!("  cargo run -- dump-frame [path]");
}
