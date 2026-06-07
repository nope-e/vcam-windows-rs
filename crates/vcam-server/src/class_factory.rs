use std::ffi::c_void;
use std::ptr::null_mut;

use windows::core::{implement, IUnknown, Result, GUID};
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_NOINTERFACE};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows_core::Interface;

use crate::debug_log;
use crate::activate::StaticCameraActivate;
use crate::constants::ACTIVATE_CLSID;

#[implement(IClassFactory)]
struct CameraActivateClassFactory;

impl IClassFactory_Impl for CameraActivateClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Option<&IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        debug_log("IClassFactory::CreateInstance");
        unsafe {
            if ppvobject.is_null() {
                return Err(windows::Win32::Foundation::E_POINTER.into());
            }
            *ppvobject = null_mut();
        }

        if punkouter.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }

        let activate = StaticCameraActivate::create()?;
        unsafe {
            if *riid == windows::Win32::Media::MediaFoundation::IMFActivate::IID {
                *ppvobject = activate.into_raw() as *mut c_void;
                return Ok(());
            }
            if *riid == IUnknown::IID {
                *ppvobject = activate.cast::<IUnknown>()?.into_raw() as *mut c_void;
                return Ok(());
            }
        }

        Err(E_NOINTERFACE.into())
    }

    fn LockServer(&self, _flock: windows::Win32::Foundation::BOOL) -> Result<()> {
        Ok(())
    }
}

pub fn dll_get_class_object(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> windows::core::HRESULT {
    debug_log("class_factory::dll_get_class_object");
    unsafe {
        if ppv.is_null() || rclsid.is_null() || riid.is_null() {
            return windows::Win32::Foundation::E_POINTER;
        }
        *ppv = null_mut();
        if *rclsid != ACTIVATE_CLSID {
            return windows::Win32::Foundation::CLASS_E_CLASSNOTAVAILABLE;
        }
    }

    let factory: IClassFactory = CameraActivateClassFactory.into();
    unsafe {
        if *riid == IClassFactory::IID {
            *ppv = factory.into_raw() as *mut c_void;
            return windows::Win32::Foundation::S_OK;
        }
        if *riid == IUnknown::IID {
            *ppv = factory.cast::<IUnknown>().expect("IClassFactory must cast to IUnknown").into_raw()
                as *mut c_void;
            return windows::Win32::Foundation::S_OK;
        }
    }

    windows::Win32::Foundation::E_NOINTERFACE
}
