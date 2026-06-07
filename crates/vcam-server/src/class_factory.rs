use std::ffi::c_void;
use std::ptr::null_mut;

use windows::core::{implement, GUID, IUnknown, Result};
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_NOINTERFACE};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows_core::Interface;

use crate::activate::StaticCameraActivate;
use crate::broker::{FrameBroker, IVcamFrameBroker};
use crate::constants::{ACTIVATE_CLSID, FRAME_BROKER_CLSID};
use crate::debug_log;

#[implement(IClassFactory)]
struct CameraActivateClassFactory;

#[implement(IClassFactory)]
struct FrameBrokerClassFactory;

impl IClassFactory_Impl for CameraActivateClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Option<&IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        debug_log("CameraActivateClassFactory::CreateInstance");
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

impl IClassFactory_Impl for FrameBrokerClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Option<&IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        debug_log("FrameBrokerClassFactory::CreateInstance");
        unsafe {
            if ppvobject.is_null() {
                return Err(windows::Win32::Foundation::E_POINTER.into());
            }
            *ppvobject = null_mut();
        }

        if punkouter.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }

        let broker = FrameBroker::create();
        unsafe {
            if *riid == IVcamFrameBroker::IID {
                *ppvobject = broker.into_raw() as *mut c_void;
                return Ok(());
            }
            if *riid == IUnknown::IID {
                *ppvobject = broker.cast::<IUnknown>()?.into_raw() as *mut c_void;
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
    }

    let factory: IClassFactory = unsafe {
        if *rclsid == ACTIVATE_CLSID {
            CameraActivateClassFactory.into()
        } else if *rclsid == FRAME_BROKER_CLSID {
            FrameBrokerClassFactory.into()
        } else {
            return windows::Win32::Foundation::CLASS_E_CLASSNOTAVAILABLE;
        }
    };

    unsafe {
        if *riid == IClassFactory::IID {
            *ppv = factory.into_raw() as *mut c_void;
            return windows::Win32::Foundation::S_OK;
        }
        if *riid == IUnknown::IID {
            *ppv = factory
                .cast::<IUnknown>()
                .expect("IClassFactory must cast to IUnknown")
                .into_raw() as *mut c_void;
            return windows::Win32::Foundation::S_OK;
        }
    }

    windows::Win32::Foundation::E_NOINTERFACE
}
