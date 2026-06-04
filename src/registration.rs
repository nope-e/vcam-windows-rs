use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows::core::{Error, PCWSTR, Result};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::SystemServices::IMAGE_DOS_HEADER;

use crate::constants::{ACTIVATE_CLSID_STRING, FRIENDLY_NAME};

unsafe extern "C" {
    static __ImageBase: IMAGE_DOS_HEADER;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryScope {
    CurrentUser,
    LocalMachine,
}

impl RegistryScope {
    fn root_key(self) -> HKEY {
        match self {
            Self::CurrentUser => HKEY_CURRENT_USER,
            Self::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

pub fn dll_register_server() -> Result<()> {
    register_server(RegistryScope::CurrentUser, None)
}

pub fn dll_unregister_server() -> Result<()> {
    unregister_server(RegistryScope::CurrentUser)
}

pub fn register_server(scope: RegistryScope, module_path: Option<&Path>) -> Result<()> {
    let module_path = match module_path {
        Some(path) => path.to_path_buf(),
        None => current_module_path()?,
    };
    let clsid_path = format!("Software\\Classes\\CLSID\\{ACTIVATE_CLSID_STRING}");
    let inproc_path = format!("{clsid_path}\\InprocServer32");
    let root = scope.root_key();

    write_reg_sz(root, &clsid_path, None, FRIENDLY_NAME)?;
    write_reg_sz(root, &inproc_path, None, &module_path.to_string_lossy())?;
    write_reg_sz(root, &inproc_path, Some("ThreadingModel"), "Both")?;
    Ok(())
}

pub fn unregister_server(scope: RegistryScope) -> Result<()> {
    let clsid_path = format!("Software\\Classes\\CLSID\\{ACTIVATE_CLSID_STRING}");
    let clsid_path_w = wide_null(&clsid_path);
    unsafe {
        let status = RegDeleteTreeW(scope.root_key(), PCWSTR(clsid_path_w.as_ptr()));
        if status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            status.ok()
        }
    }
}

fn current_module_path() -> Result<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    let module = windows::Win32::Foundation::HMODULE(&raw const __ImageBase as *const _ as *mut c_void);
    let len = unsafe { GetModuleFileNameW(module, &mut buffer) } as usize;

    if len == 0 {
        return Err(Error::from_win32());
    }

    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
}

fn write_reg_sz(root: HKEY, path: &str, name: Option<&str>, value: &str) -> Result<()> {
    let path_w = wide_null(path);
    let name_w = name.map(wide_null);
    let value_w = wide_null(value);
    let mut key = HKEY::default();

    unsafe {
        RegCreateKeyExW(
            root,
            PCWSTR(path_w.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()?;

        let bytes = std::slice::from_raw_parts(
            value_w.as_ptr() as *const u8,
            value_w.len() * std::mem::size_of::<u16>(),
        );
        let result = RegSetValueExW(
            key,
            name_w
                .as_ref()
                .map(|value| PCWSTR(value.as_ptr()))
                .unwrap_or(PCWSTR::null()),
            0,
            REG_SZ,
            Some(bytes),
        );
        let close_result = RegCloseKey(key);
        result.ok()?;
        close_result.ok()?;
    }

    Ok(())
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
