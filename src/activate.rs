use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::Mutex;

use windows::core::{implement, GUID, IUnknown, PCWSTR, PWSTR, Result};
use windows::Win32::Foundation::{E_NOINTERFACE, E_POINTER};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFActivate_Impl, IMFAttributes, IMFAttributes_Impl, IMFMediaSource,
    IMFMediaSourceEx, MFCreateAttributes, MF_ATTRIBUTE_TYPE, MF_ATTRIBUTES_MATCH_TYPE,
};
use windows_core::{Interface, PROPVARIANT};

use crate::media_source::StaticImageMediaSource;

#[implement(IMFActivate)]
pub struct StaticCameraActivate {
    attributes: IMFAttributes,
    active_source: Mutex<Option<IMFMediaSourceEx>>,
}

impl StaticCameraActivate {
    pub fn create() -> Result<IMFActivate> {
        let attributes = create_attributes(8)?;
        Ok(Self {
            attributes,
            active_source: Mutex::new(None),
        }
        .into())
    }
}

impl IMFActivate_Impl for StaticCameraActivate_Impl {
    fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        unsafe {
            if riid.is_null() || ppv.is_null() {
                return Err(E_POINTER.into());
            }
            *ppv = null_mut();
        }

        let source = StaticImageMediaSource::create()?;
        *self.active_source.lock().expect("activate state poisoned") = Some(source.clone());

        unsafe {
            if *riid == IMFMediaSourceEx::IID {
                *ppv = source.into_raw() as *mut c_void;
                return Ok(());
            }
            if *riid == IMFMediaSource::IID {
                *ppv = source.cast::<IMFMediaSource>()?.into_raw() as *mut c_void;
                return Ok(());
            }
            if *riid == IUnknown::IID {
                *ppv = source.cast::<IUnknown>()?.into_raw() as *mut c_void;
                return Ok(());
            }
        }

        Err(E_NOINTERFACE.into())
    }

    fn ShutdownObject(&self) -> Result<()> {
        if let Some(source) = self
            .active_source
            .lock()
            .expect("activate state poisoned")
            .take()
        {
            unsafe {
                source.cast::<IMFMediaSource>()?.Shutdown()?;
            }
        }
        Ok(())
    }

    fn DetachObject(&self) -> Result<()> {
        self.active_source
            .lock()
            .expect("activate state poisoned")
            .take();
        Ok(())
    }
}

impl IMFAttributes_Impl for StaticCameraActivate_Impl {
    fn GetItem(&self, guidkey: *const GUID, pvalue: *mut PROPVARIANT) -> Result<()> {
        unsafe { self.attributes.GetItem(guidkey, if pvalue.is_null() { None } else { Some(pvalue) }) }
    }

    fn GetItemType(&self, guidkey: *const GUID) -> Result<MF_ATTRIBUTE_TYPE> {
        unsafe { self.attributes.GetItemType(guidkey) }
    }

    fn CompareItem(&self, guidkey: *const GUID, value: *const PROPVARIANT) -> Result<windows::Win32::Foundation::BOOL> {
        unsafe { self.attributes.CompareItem(guidkey, value) }
    }

    fn Compare(
        &self,
        ptheirs: Option<&IMFAttributes>,
        matchtype: MF_ATTRIBUTES_MATCH_TYPE,
    ) -> Result<windows::Win32::Foundation::BOOL> {
        let theirs = ptheirs.ok_or(E_POINTER)?;
        unsafe { self.attributes.Compare(theirs, matchtype) }
    }

    fn GetUINT32(&self, guidkey: *const GUID) -> Result<u32> {
        unsafe { self.attributes.GetUINT32(guidkey) }
    }

    fn GetUINT64(&self, guidkey: *const GUID) -> Result<u64> {
        unsafe { self.attributes.GetUINT64(guidkey) }
    }

    fn GetDouble(&self, guidkey: *const GUID) -> Result<f64> {
        unsafe { self.attributes.GetDouble(guidkey) }
    }

    fn GetGUID(&self, guidkey: *const GUID) -> Result<GUID> {
        unsafe { self.attributes.GetGUID(guidkey) }
    }

    fn GetStringLength(&self, guidkey: *const GUID) -> Result<u32> {
        unsafe { self.attributes.GetStringLength(guidkey) }
    }

    fn GetString(
        &self,
        guidkey: *const GUID,
        pwszvalue: PWSTR,
        cchbufsize: u32,
        pcchlength: *mut u32,
    ) -> Result<()> {
        if pwszvalue.0.is_null() {
            return Err(E_POINTER.into());
        }
        let buffer = unsafe { std::slice::from_raw_parts_mut(pwszvalue.0, cchbufsize as usize) };
        unsafe {
            self.attributes.GetString(
                guidkey,
                buffer,
                if pcchlength.is_null() {
                    None
                } else {
                    Some(pcchlength)
                },
            )
        }
    }

    fn GetAllocatedString(
        &self,
        guidkey: *const GUID,
        ppwszvalue: *mut PWSTR,
        pcchlength: *mut u32,
    ) -> Result<()> {
        unsafe { self.attributes.GetAllocatedString(guidkey, ppwszvalue, pcchlength) }
    }

    fn GetBlobSize(&self, guidkey: *const GUID) -> Result<u32> {
        unsafe { self.attributes.GetBlobSize(guidkey) }
    }

    fn GetBlob(
        &self,
        guidkey: *const GUID,
        pbuf: *mut u8,
        cbbufsize: u32,
        pcbblobsize: *mut u32,
    ) -> Result<()> {
        if pbuf.is_null() {
            return Err(E_POINTER.into());
        }
        let buffer = unsafe { std::slice::from_raw_parts_mut(pbuf, cbbufsize as usize) };
        unsafe {
            self.attributes.GetBlob(
                guidkey,
                buffer,
                if pcbblobsize.is_null() {
                    None
                } else {
                    Some(pcbblobsize)
                },
            )
        }
    }

    fn GetAllocatedBlob(
        &self,
        guidkey: *const GUID,
        ppbuf: *mut *mut u8,
        pcbsize: *mut u32,
    ) -> Result<()> {
        unsafe { self.attributes.GetAllocatedBlob(guidkey, ppbuf, pcbsize) }
    }

    fn GetUnknown(&self, guidkey: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        unsafe {
            if riid.is_null() || ppv.is_null() {
                return Err(E_POINTER.into());
            }
            (Interface::vtable(&self.attributes).GetUnknown)(
                Interface::as_raw(&self.attributes),
                guidkey,
                riid,
                ppv,
            )
            .ok()
        }
    }

    fn SetItem(&self, guidkey: *const GUID, value: *const PROPVARIANT) -> Result<()> {
        unsafe { self.attributes.SetItem(guidkey, value) }
    }

    fn DeleteItem(&self, guidkey: *const GUID) -> Result<()> {
        unsafe { self.attributes.DeleteItem(guidkey) }
    }

    fn DeleteAllItems(&self) -> Result<()> {
        unsafe { self.attributes.DeleteAllItems() }
    }

    fn SetUINT32(&self, guidkey: *const GUID, unvalue: u32) -> Result<()> {
        unsafe { self.attributes.SetUINT32(guidkey, unvalue) }
    }

    fn SetUINT64(&self, guidkey: *const GUID, unvalue: u64) -> Result<()> {
        unsafe { self.attributes.SetUINT64(guidkey, unvalue) }
    }

    fn SetDouble(&self, guidkey: *const GUID, fvalue: f64) -> Result<()> {
        unsafe { self.attributes.SetDouble(guidkey, fvalue) }
    }

    fn SetGUID(&self, guidkey: *const GUID, guidvalue: *const GUID) -> Result<()> {
        unsafe { self.attributes.SetGUID(guidkey, guidvalue) }
    }

    fn SetString(&self, guidkey: *const GUID, wszvalue: &PCWSTR) -> Result<()> {
        unsafe { self.attributes.SetString(guidkey, *wszvalue) }
    }

    fn SetBlob(&self, guidkey: *const GUID, pbuf: *const u8, cbbufsize: u32) -> Result<()> {
        unsafe { self.attributes.SetBlob(guidkey, std::slice::from_raw_parts(pbuf, cbbufsize as usize)) }
    }

    fn SetUnknown(&self, guidkey: *const GUID, punknown: Option<&IUnknown>) -> Result<()> {
        unsafe { self.attributes.SetUnknown(guidkey, punknown) }
    }

    fn LockStore(&self) -> Result<()> {
        unsafe { self.attributes.LockStore() }
    }

    fn UnlockStore(&self) -> Result<()> {
        unsafe { self.attributes.UnlockStore() }
    }

    fn GetCount(&self) -> Result<u32> {
        unsafe { self.attributes.GetCount() }
    }

    fn GetItemByIndex(
        &self,
        unindex: u32,
        pguidkey: *mut GUID,
        pvalue: *mut PROPVARIANT,
    ) -> Result<()> {
        unsafe { self.attributes.GetItemByIndex(unindex, pguidkey, if pvalue.is_null() { None } else { Some(pvalue) }) }
    }

    fn CopyAllItems(&self, pdest: Option<&IMFAttributes>) -> Result<()> {
        let dest = pdest.ok_or(E_POINTER)?;
        unsafe { self.attributes.CopyAllItems(dest) }
    }
}

fn create_attributes(initial_size: u32) -> Result<IMFAttributes> {
    let mut attributes = None;
    unsafe {
        MFCreateAttributes(&mut attributes, initial_size)?;
    }
    attributes.ok_or_else(|| E_POINTER.into())
}
