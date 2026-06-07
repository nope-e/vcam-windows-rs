use windows::core::{implement, Result};
use windows::Win32::Foundation::{BOOL, E_POINTER};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows_core::{IUnknown, IUnknown_Vtbl};

use crate::debug_log;
use crate::feed_shared::{
    query_feed_state, reset_feed_session, start_feed_session, stop_feed_session, VCAM_FEED_CONFIG,
    VCAM_FEED_STATE,
};
use crate::constants::FRAME_BROKER_CLSID;

#[windows_core::interface("F57C3F03-9418-4E22-9D16-B29E4A4AC4B6")]
pub unsafe trait IVcamFrameBroker: IUnknown {
    pub fn StartSession(&self, config: *const VCAM_FEED_CONFIG, force_reset: BOOL) -> Result<()>;
    pub fn StopSession(&self) -> Result<()>;
    pub fn GetSessionState(&self, state: *mut VCAM_FEED_STATE) -> Result<()>;
    pub fn ResetSession(&self) -> Result<()>;
}

#[implement(IVcamFrameBroker)]
pub struct FrameBroker {
    _private: (),
}

impl FrameBroker {
    pub fn create() -> IVcamFrameBroker {
        Self { _private: () }.into()
    }
}

impl IVcamFrameBroker_Impl for FrameBroker_Impl {
    unsafe fn StartSession(&self, config: *const VCAM_FEED_CONFIG, force_reset: BOOL) -> Result<()> {
        debug_log("IVcamFrameBroker::StartSession");
        if config.is_null() {
            return Err(E_POINTER.into());
        }
        start_feed_session(unsafe { *config }, force_reset.as_bool())
    }

    unsafe fn StopSession(&self) -> Result<()> {
        debug_log("IVcamFrameBroker::StopSession");
        stop_feed_session()
    }

    unsafe fn GetSessionState(&self, state: *mut VCAM_FEED_STATE) -> Result<()> {
        debug_log("IVcamFrameBroker::GetSessionState");
        if state.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *state = query_feed_state()?;
        }
        Ok(())
    }

    unsafe fn ResetSession(&self) -> Result<()> {
        debug_log("IVcamFrameBroker::ResetSession");
        reset_feed_session()
    }
}

pub fn create_frame_broker() -> Result<IVcamFrameBroker> {
    debug_log("create_frame_broker");
    unsafe { CoCreateInstance(&FRAME_BROKER_CLSID, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
}
