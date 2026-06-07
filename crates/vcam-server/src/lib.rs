#![allow(non_snake_case)]

use std::ffi::c_void;
use std::io::Write;

use windows::core::{GUID, HRESULT};
use windows::Win32::Foundation::{S_FALSE, S_OK};

mod activate;
mod broker;
mod class_factory;
mod constants;
mod feed_shared;
mod media_event;
mod media_source;
mod media_stream;
pub mod registration;
mod test_pattern;
mod video_format;

pub use broker::{create_frame_broker, IVcamFrameBroker};
pub use constants::{
    ACTIVATE_CLSID, ACTIVATE_CLSID_STRING, FRAME_BROKER_CLSID, FRAME_BROKER_CLSID_STRING,
    FRIENDLY_NAME,
};
pub use feed_shared::{
    feed_control_file_path, feed_data_file_path, feed_mutex_name, feed_shared_root_path,
    query_feed_state, reset_feed_session, start_feed_session, stop_feed_session,
    try_active_video_format, FeedFrame, FeedSessionProducer, FeedSessionReader,
    VCAM_FEED_CONFIG, VCAM_FEED_STATE, VcamSharedFeedControl, VcamSharedFeedSlotHeader,
};
pub use test_pattern::{
    bgra_to_nv12_bytes, copy_bgra_to_surface, copy_nv12_to_surface, nv12_to_bgra_bytes,
    validate_dump_path, write_bgra_bmp, write_bgra_bmp_for_format, write_nv12_bmp,
    write_nv12_bmp_for_format, StaticTestPattern,
};
pub use video_format::VideoFormat;

pub fn debug_log(message: &str) {
    if std::env::var_os("VCAM_TRACE").is_none() {
        return;
    }

    eprintln!("[vcam] {message}");
    let _ = std::io::stderr().flush();
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    debug_log("DllGetClassObject");
    class_factory::dll_get_class_object(rclsid, riid, ppv)
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[unsafe(no_mangle)]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    debug_log("DllRegisterServer");
    match registration::dll_register_server() {
        Ok(()) => S_OK,
        Err(err) => err.code(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    debug_log("DllUnregisterServer");
    match registration::dll_unregister_server() {
        Ok(()) => S_OK,
        Err(err) => err.code(),
    }
}
