use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::{addr_of, addr_of_mut, copy_nonoverlapping, read_volatile, write_bytes, write_volatile};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::{Error, PCWSTR, Result};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, BOOL, E_FAIL, E_INVALIDARG, ERROR_ALREADY_EXISTS, ERROR_BUSY,
    ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, HANDLE, HLOCAL, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Storage::FileSystem::{
    CreateDirectoryW, CreateFileW, GetFileSizeEx, SetEndOfFile, SetFilePointerEx,
    FILE_ATTRIBUTE_NORMAL, FILE_BEGIN, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS, OPEN_EXISTING,
};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MapViewOfFile, PAGE_READONLY,
    PAGE_READWRITE, UnmapViewOfFile, MEMORY_MAPPED_VIEW_ADDRESS,
};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject, INFINITE};

use crate::constants::{
    ACTIVATE_CLSID_STRING, VCAM_FEED_INPUT_FORMAT_BGRA8, VCAM_FEED_MAGIC, VCAM_FEED_SLOT_COUNT,
    VCAM_FEED_VERSION, VCAM_INVALID_SLOT_INDEX,
};
use crate::debug_log;
use crate::video_format::VideoFormat;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VCAM_FEED_CONFIG {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub input_format: u32,
    pub input_stride: u32,
}

impl VCAM_FEED_CONFIG {
    pub fn bgra8(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Result<Self> {
        let format = VideoFormat::new(width, height, fps_num, fps_den)?;
        Ok(Self::from_video_format(format))
    }

    pub fn from_video_format(format: VideoFormat) -> Self {
        Self {
            width: format.width,
            height: format.height,
            fps_num: format.fps_num,
            fps_den: format.fps_den,
            input_format: VCAM_FEED_INPUT_FORMAT_BGRA8,
            input_stride: format.bgra_stride() as u32,
        }
    }

    pub fn validate(self) -> Result<()> {
        let format = self.video_format()?;
        if self.input_format != VCAM_FEED_INPUT_FORMAT_BGRA8 {
            return Err(Error::new(
                E_INVALIDARG.into(),
                "only BGRA8 shared-memory input is supported",
            ));
        }
        if self.input_stride < format.bgra_stride() as u32 {
            return Err(Error::new(
                E_INVALIDARG.into(),
                "input stride is smaller than width * 4",
            ));
        }
        self.payload_bytes()?;
        Ok(())
    }

    pub fn video_format(self) -> Result<VideoFormat> {
        VideoFormat::new(self.width, self.height, self.fps_num, self.fps_den)
    }

    pub fn payload_bytes(self) -> Result<usize> {
        self.validate_stride_only()?;
        (self.input_stride as usize)
            .checked_mul(self.height as usize)
            .ok_or_else(|| Error::new(E_INVALIDARG.into(), "payload byte count overflow"))
    }

    fn validate_stride_only(self) -> Result<()> {
        self.video_format()?;
        if self.input_format != VCAM_FEED_INPUT_FORMAT_BGRA8 {
            return Err(Error::new(
                E_INVALIDARG.into(),
                "only BGRA8 shared-memory input is supported",
            ));
        }
        if self.input_stride == 0 {
            return Err(Error::new(E_INVALIDARG.into(), "input stride must be non-zero"));
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VCAM_FEED_STATE {
    pub active: u32,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub input_format: u32,
    pub input_stride: u32,
    pub slot_count: u32,
    pub committed_frame_id: u64,
    pub last_timestamp_100ns: i64,
}

impl VCAM_FEED_STATE {
    pub fn is_active(&self) -> bool {
        self.active != 0
    }

    pub fn config(&self) -> Result<VCAM_FEED_CONFIG> {
        let config = VCAM_FEED_CONFIG {
            width: self.width,
            height: self.height,
            fps_num: self.fps_num,
            fps_den: self.fps_den,
            input_format: self.input_format,
            input_stride: self.input_stride,
        };
        config.validate()?;
        Ok(config)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VcamSharedFeedControl {
    pub magic: u32,
    pub version: u32,
    pub active: u32,
    pub reserved0: u32,
    pub config: VCAM_FEED_CONFIG,
    pub slot_count: u32,
    pub slot_bytes: u32,
    pub committed_slot: u32,
    pub reserved1: u32,
    pub committed_frame_id: u64,
    pub last_timestamp_100ns: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VcamSharedFeedSlotHeader {
    pub frame_id: u64,
    pub timestamp_100ns: i64,
    pub bytes_written: u32,
    pub reserved: u32,
}

#[derive(Clone)]
pub struct FeedFrame {
    pub frame_id: u64,
    pub timestamp_100ns: i64,
    pub bgra: Arc<[u8]>,
}

const FEED_STALE_GRACE_MS_MIN: u64 = 1_500;

pub fn feed_shared_root_path() -> PathBuf {
    let public_root = std::env::var_os("PUBLIC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
    public_root.join("Documents").join("vcam-windows-rs")
}

pub fn feed_control_file_path() -> PathBuf {
    feed_shared_root_path().join(format!("RustVcamFeedControl-{ACTIVATE_CLSID_STRING}.bin"))
}

pub fn feed_data_file_path() -> PathBuf {
    feed_shared_root_path().join(format!("RustVcamFeedData-{ACTIVATE_CLSID_STRING}.bin"))
}

pub fn feed_mutex_name() -> String {
    format!("Global\\RustVcamFeedMutex-{ACTIVATE_CLSID_STRING}")
}

pub fn query_feed_state() -> Result<VCAM_FEED_STATE> {
    let mutex = NamedMutex::open_or_create()?;
    let _guard = mutex.lock()?;
    Ok(read_state_locked()?)
}

pub fn try_active_video_format() -> Result<Option<VideoFormat>> {
    let state = query_feed_state()?;
    if !state.is_active() {
        return Ok(None);
    }
    Ok(Some(state.config()?.video_format()?))
}

pub fn start_feed_session(config: VCAM_FEED_CONFIG, force_reset: bool) -> Result<()> {
    config.validate()?;
    let payload_bytes = config.payload_bytes()?;
    let data_bytes = slot_region_bytes(payload_bytes)?
        .checked_mul(VCAM_FEED_SLOT_COUNT as usize)
        .ok_or_else(|| Error::new(E_INVALIDARG.into(), "data mapping size overflow"))?;

    let mutex = NamedMutex::open_or_create()?;
    let _guard = mutex.lock()?;

    let control = MappedMemory::create_or_open(&feed_control_file_path(), size_of::<VcamSharedFeedControl>())?;
    let data = MappedMemory::create_or_open(&feed_data_file_path(), data_bytes)?;
    let control_ptr = control.view.as_mut_ptr::<VcamSharedFeedControl>();

    if is_valid_control(control_ptr) {
        deactivate_stale_session_locked(control_ptr);
    }
    if is_valid_control(control_ptr) && read_control_active_bit(control_ptr) && !force_reset {
        return Err(Error::new(
            ERROR_BUSY.to_hresult(),
            "a shared-memory feed session is already active",
        ));
    }

    unsafe {
        write_bytes(control.view.as_mut_ptr::<u8>(), 0, size_of::<VcamSharedFeedControl>());
        write_bytes(data.view.as_mut_ptr::<u8>(), 0, data_bytes);
    }
    write_control_header(control_ptr, &config, payload_bytes as u32);
    *session_keepalive()
        .lock()
        .expect("feed keepalive mutex poisoned") = Some(SessionKeepAlive {
        _control: control,
        _data: data,
    });
    debug_log("start_feed_session initialized shared-memory session");
    Ok(())
}

pub fn stop_feed_session() -> Result<()> {
    let mutex = NamedMutex::open_or_create()?;
    let _guard = mutex.lock()?;

    let Some(control) =
        MappedMemory::try_open(&feed_control_file_path(), size_of::<VcamSharedFeedControl>(), true)?
    else {
        return Ok(());
    };
    let control_ptr = control.view.as_mut_ptr::<VcamSharedFeedControl>();
    if is_valid_control(control_ptr) {
        unsafe {
            write_volatile(addr_of_mut!((*control_ptr).active), 0);
            write_volatile(addr_of_mut!((*control_ptr).reserved0), 0);
            write_volatile(addr_of_mut!((*control_ptr).reserved1), 0);
        }
    }
    session_keepalive()
        .lock()
        .expect("feed keepalive mutex poisoned")
        .take();
    Ok(())
}

pub fn reset_feed_session() -> Result<()> {
    let mutex = NamedMutex::open_or_create()?;
    let _guard = mutex.lock()?;

    let Some(control) =
        MappedMemory::try_open(&feed_control_file_path(), size_of::<VcamSharedFeedControl>(), true)?
    else {
        return Ok(());
    };
    let control_ptr = control.view.as_mut_ptr::<VcamSharedFeedControl>();
    let data_size = if is_valid_control(control_ptr) {
        slot_region_bytes(read_control_slot_bytes(control_ptr) as usize)?
            .checked_mul(read_control_slot_count(control_ptr) as usize)
            .ok_or_else(|| Error::new(E_INVALIDARG.into(), "data mapping size overflow"))?
    } else {
        0
    };

    if data_size != 0 {
        if let Some(data) = MappedMemory::try_open(&feed_data_file_path(), data_size, true)? {
            unsafe {
                write_bytes(data.view.as_mut_ptr::<u8>(), 0, data_size);
            }
        }
    }

    unsafe {
        write_bytes(control.view.as_mut_ptr::<u8>(), 0, size_of::<VcamSharedFeedControl>());
    }
    session_keepalive()
        .lock()
        .expect("feed keepalive mutex poisoned")
        .take();
    Ok(())
}

pub struct FeedSessionProducer {
    _control: MappedMemory,
    _data: MappedMemory,
    control_ptr: *mut VcamSharedFeedControl,
    data_ptr: *mut u8,
    config: VCAM_FEED_CONFIG,
    payload_bytes: usize,
    slot_span_bytes: usize,
}

impl FeedSessionProducer {
    pub fn open(config: VCAM_FEED_CONFIG) -> Result<Self> {
        config.validate()?;
        let payload_bytes = config.payload_bytes()?;
        let slot_span_bytes = slot_region_bytes(payload_bytes)?;
        let control = MappedMemory::open(&feed_control_file_path(), size_of::<VcamSharedFeedControl>(), true)?;
        let data = MappedMemory::open(
            &feed_data_file_path(),
            slot_span_bytes
                .checked_mul(VCAM_FEED_SLOT_COUNT as usize)
                .ok_or_else(|| Error::new(E_INVALIDARG.into(), "data mapping size overflow"))?,
            true,
        )?;
        let control_ptr = control.view.as_mut_ptr::<VcamSharedFeedControl>();
        if !is_valid_control(control_ptr) {
            return Err(Error::new(E_FAIL.into(), "shared-memory control header is not initialized"));
        }
        let header_config = read_control_config(control_ptr);
        if header_config != config {
            return Err(Error::new(
                E_FAIL.into(),
                "shared-memory session config does not match the requested producer config",
            ));
        }

        Ok(Self {
            control_ptr,
            data_ptr: data.view.as_mut_ptr::<u8>(),
            _control: control,
            _data: data,
            config,
            payload_bytes,
            slot_span_bytes,
        })
    }

    pub fn publish_bgra_frame(
        &mut self,
        frame_id: u64,
        timestamp_100ns: i64,
        payload: &[u8],
    ) -> Result<()> {
        if payload.len() != self.payload_bytes {
            return Err(Error::new(
                E_INVALIDARG.into(),
                format!(
                    "frame payload size mismatch: expected {} bytes, got {}",
                    self.payload_bytes,
                    payload.len()
                ),
            ));
        }
        if !read_control_active_bit(self.control_ptr) {
            return Err(Error::new(E_FAIL.into(), "shared-memory feed session is inactive"));
        }

        let slot_index = (frame_id % VCAM_FEED_SLOT_COUNT as u64) as usize;
        let slot_base = unsafe { self.data_ptr.add(slot_index * self.slot_span_bytes) };
        let slot_header = slot_base.cast::<VcamSharedFeedSlotHeader>();
        let payload_ptr = unsafe { slot_base.add(size_of::<VcamSharedFeedSlotHeader>()) };

        unsafe {
            copy_nonoverlapping(payload.as_ptr(), payload_ptr, payload.len());
            std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
            write_volatile(addr_of_mut!((*slot_header).bytes_written), payload.len() as u32);
            write_volatile(addr_of_mut!((*slot_header).timestamp_100ns), timestamp_100ns);
            write_volatile(addr_of_mut!((*slot_header).frame_id), frame_id);
            std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
            write_control_heartbeat_ms(self.control_ptr, current_unix_time_ms());
            write_volatile(addr_of_mut!((*self.control_ptr).last_timestamp_100ns), timestamp_100ns);
            write_volatile(addr_of_mut!((*self.control_ptr).committed_slot), slot_index as u32);
            write_volatile(addr_of_mut!((*self.control_ptr).committed_frame_id), frame_id);
        }
        Ok(())
    }

    pub fn config(&self) -> VCAM_FEED_CONFIG {
        self.config
    }
}

pub struct FeedSessionReader {
    _control: MappedMemory,
    _data: MappedMemory,
    control_ptr: *const VcamSharedFeedControl,
    data_ptr: *const u8,
    config: VCAM_FEED_CONFIG,
    payload_bytes: usize,
    slot_span_bytes: usize,
}

impl FeedSessionReader {
    pub fn open_active() -> Result<Option<Self>> {
        let state = query_feed_state()?;
        if !state.is_active() {
            return Ok(None);
        }
        let config = state.config()?;
        Ok(Some(Self::open(config)?))
    }

    pub fn open(config: VCAM_FEED_CONFIG) -> Result<Self> {
        config.validate()?;
        let payload_bytes = config.payload_bytes()?;
        let slot_span_bytes = slot_region_bytes(payload_bytes)?;
        let control = MappedMemory::open(&feed_control_file_path(), size_of::<VcamSharedFeedControl>(), false)?;
        let data = MappedMemory::open(
            &feed_data_file_path(),
            slot_span_bytes
                .checked_mul(VCAM_FEED_SLOT_COUNT as usize)
                .ok_or_else(|| Error::new(E_INVALIDARG.into(), "data mapping size overflow"))?,
            false,
        )?;
        let control_ptr = control.view.as_ptr::<VcamSharedFeedControl>();
        if !is_valid_control(control_ptr) {
            return Err(Error::new(E_FAIL.into(), "shared-memory control header is not initialized"));
        }
        let header_config = read_control_config(control_ptr);
        if header_config != config {
            return Err(Error::new(
                E_FAIL.into(),
                "shared-memory session config does not match the requested reader config",
            ));
        }

        Ok(Self {
            control_ptr,
            data_ptr: data.view.as_ptr::<u8>(),
            _control: control,
            _data: data,
            config,
            payload_bytes,
            slot_span_bytes,
        })
    }

    pub fn config(&self) -> VCAM_FEED_CONFIG {
        self.config
    }

    pub fn is_active(&self) -> bool {
        read_control_active(self.control_ptr)
    }

    pub fn try_read_latest_frame(&self) -> Result<Option<FeedFrame>> {
        if !read_control_active(self.control_ptr) {
            return Ok(None);
        }

        for _ in 0..2 {
            let committed_frame_id_before = read_control_committed_frame_id(self.control_ptr);
            let committed_slot = read_control_committed_slot(self.control_ptr);
            if committed_slot == VCAM_INVALID_SLOT_INDEX {
                return Ok(None);
            }

            let frame = self.copy_slot_frame(committed_slot as usize, committed_frame_id_before)?;
            let committed_frame_id_after = read_control_committed_frame_id(self.control_ptr);
            if committed_frame_id_before == committed_frame_id_after {
                return Ok(Some(frame));
            }
        }

        Ok(None)
    }

    fn copy_slot_frame(&self, slot_index: usize, expected_frame_id: u64) -> Result<FeedFrame> {
        let slot_base = unsafe { self.data_ptr.add(slot_index * self.slot_span_bytes) };
        let slot_header = slot_base.cast::<VcamSharedFeedSlotHeader>();
        let payload_ptr = unsafe { slot_base.add(size_of::<VcamSharedFeedSlotHeader>()) };
        let frame_id = unsafe { read_volatile(addr_of!((*slot_header).frame_id)) };
        let timestamp_100ns = unsafe { read_volatile(addr_of!((*slot_header).timestamp_100ns)) };
        let bytes_written = unsafe { read_volatile(addr_of!((*slot_header).bytes_written)) as usize };

        if frame_id != expected_frame_id {
            return Err(Error::new(E_FAIL.into(), "slot frame id changed during copy"));
        }
        if bytes_written != self.payload_bytes {
            return Err(Error::new(
                E_FAIL.into(),
                "slot payload size does not match the configured feed size",
            ));
        }

        let format = self.config.video_format()?;
        let tight_stride = format.bgra_stride();
        let source_stride = self.config.input_stride as usize;
        let mut bgra = vec![0u8; format.bgra_frame_bytes()];

        unsafe {
            if source_stride == tight_stride {
                copy_nonoverlapping(payload_ptr, bgra.as_mut_ptr(), bgra.len());
            } else {
                for row in 0..format.height as usize {
                    let src_row = payload_ptr.add(row * source_stride);
                    let dst_row = bgra.as_mut_ptr().add(row * tight_stride);
                    copy_nonoverlapping(src_row, dst_row, tight_stride);
                }
            }
        }

        Ok(FeedFrame {
            frame_id,
            timestamp_100ns,
            bgra: Arc::<[u8]>::from(bgra),
        })
    }
}

struct NamedMutex {
    handle: OwnedHandle,
}

struct SessionKeepAlive {
    _control: MappedMemory,
    _data: MappedMemory,
}

impl NamedMutex {
    fn open_or_create() -> Result<Self> {
        let name = wide_null(&feed_mutex_name());
        let security = SharedSecurityAttributes::new()?;
        let handle = unsafe {
            CreateMutexW(Some(&security.attributes as *const SECURITY_ATTRIBUTES), BOOL(0), PCWSTR(name.as_ptr()))
        }?;
        Ok(Self {
            handle: OwnedHandle(handle),
        })
    }

    fn lock(&self) -> Result<NamedMutexGuard<'_>> {
        let result = unsafe { WaitForSingleObject(self.handle.0, INFINITE) };
        if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
            return Err(Error::from_win32());
        }
        Ok(NamedMutexGuard {
            handle: &self.handle,
        })
    }
}

struct NamedMutexGuard<'a> {
    handle: &'a OwnedHandle,
}

impl Drop for NamedMutexGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle.0);
        }
    }
}

struct MappedMemory {
    _file: OwnedHandle,
    _mapping: OwnedHandle,
    view: MappedView,
}

impl MappedMemory {
    fn create_or_open(path: &Path, size: usize) -> Result<Self> {
        ensure_shared_root_exists()?;
        let file = open_backing_file(path, true, true)?;
        ensure_file_size(file.0, size)?;
        Self::map_file(file, size, true)
    }

    fn open(path: &Path, size: usize, writable: bool) -> Result<Self> {
        Self::try_open(path, size, writable)?.ok_or_else(|| {
            Error::new(
                ERROR_FILE_NOT_FOUND.to_hresult(),
                "shared feed backing file does not exist",
            )
        })
    }

    fn try_open(path: &Path, size: usize, writable: bool) -> Result<Option<Self>> {
        let Some(file) = try_open_backing_file(path, writable)? else {
            return Ok(None);
        };
        if current_file_size(file.0)? < size as u64 {
            return Err(Error::new(
                E_FAIL.into(),
                format!(
                    "shared feed backing file '{}' is smaller than expected",
                    path.display()
                ),
            ));
        }
        Ok(Some(Self::map_file(file, size, writable)?))
    }

    fn map_file(file: OwnedHandle, size: usize, writable: bool) -> Result<Self> {
        let (high, low) = split_mapping_size(size);
        let mapping = unsafe {
            CreateFileMappingW(
                file.0,
                None,
                if writable { PAGE_READWRITE } else { PAGE_READONLY },
                high,
                low,
                PCWSTR::null(),
            )
        }?;
        let mapping = OwnedHandle(mapping);
        let view = MappedView::map(mapping.0, if writable { FILE_MAP_WRITE } else { FILE_MAP_READ }, size)?;
        Ok(Self {
            _file: file,
            _mapping: mapping,
            view,
        })
    }
}

struct OwnedHandle(HANDLE);

unsafe impl Send for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

struct MappedView {
    base: MEMORY_MAPPED_VIEW_ADDRESS,
}

unsafe impl Send for MappedView {}

impl MappedView {
    fn map(handle: HANDLE, access: windows::Win32::System::Memory::FILE_MAP, size: usize) -> Result<Self> {
        let base = unsafe { MapViewOfFile(handle, access, 0, 0, size) };
        if base.Value.is_null() {
            return Err(Error::from_win32());
        }
        Ok(Self { base })
    }

    fn as_ptr<T>(&self) -> *const T {
        self.base.Value.cast::<T>()
    }

    fn as_mut_ptr<T>(&self) -> *mut T {
        self.base.Value.cast::<T>()
    }
}

impl Drop for MappedView {
    fn drop(&mut self) {
        if !self.base.Value.is_null() {
            unsafe {
                let _ = UnmapViewOfFile(self.base);
            }
        }
    }
}

fn read_state_locked() -> Result<VCAM_FEED_STATE> {
    let Some(control) =
        MappedMemory::try_open(&feed_control_file_path(), size_of::<VcamSharedFeedControl>(), false)?
    else {
        return Ok(VCAM_FEED_STATE::default());
    };
    let control_ptr = control.view.as_ptr::<VcamSharedFeedControl>();
    if !is_valid_control(control_ptr) {
        return Ok(VCAM_FEED_STATE::default());
    }
    deactivate_stale_session_locked(control_ptr.cast_mut());

    let config = read_control_config(control_ptr);
    Ok(VCAM_FEED_STATE {
        active: if read_control_active(control_ptr) { 1 } else { 0 },
        width: config.width,
        height: config.height,
        fps_num: config.fps_num,
        fps_den: config.fps_den,
        input_format: config.input_format,
        input_stride: config.input_stride,
        slot_count: read_control_slot_count(control_ptr),
        committed_frame_id: read_control_committed_frame_id(control_ptr),
        last_timestamp_100ns: read_control_last_timestamp(control_ptr),
    })
}

fn write_control_header(control_ptr: *mut VcamSharedFeedControl, config: &VCAM_FEED_CONFIG, slot_bytes: u32) {
    unsafe {
        write_volatile(addr_of_mut!((*control_ptr).magic), VCAM_FEED_MAGIC);
        write_volatile(addr_of_mut!((*control_ptr).version), VCAM_FEED_VERSION);
        write_volatile(addr_of_mut!((*control_ptr).config), *config);
        write_volatile(addr_of_mut!((*control_ptr).slot_count), VCAM_FEED_SLOT_COUNT);
        write_volatile(addr_of_mut!((*control_ptr).slot_bytes), slot_bytes);
        write_volatile(addr_of_mut!((*control_ptr).committed_slot), VCAM_INVALID_SLOT_INDEX);
        write_volatile(addr_of_mut!((*control_ptr).committed_frame_id), 0);
        write_volatile(addr_of_mut!((*control_ptr).last_timestamp_100ns), 0);
        write_control_heartbeat_ms(control_ptr, current_unix_time_ms());
        write_volatile(addr_of_mut!((*control_ptr).active), 1);
    }
}

fn is_valid_control(control_ptr: *const VcamSharedFeedControl) -> bool {
    unsafe {
        read_volatile(addr_of!((*control_ptr).magic)) == VCAM_FEED_MAGIC
            && read_volatile(addr_of!((*control_ptr).version)) == VCAM_FEED_VERSION
    }
}

fn read_control_active(control_ptr: *const VcamSharedFeedControl) -> bool {
    read_control_active_bit(control_ptr) && is_control_heartbeat_fresh(control_ptr)
}

fn read_control_active_bit(control_ptr: *const VcamSharedFeedControl) -> bool {
    unsafe { read_volatile(addr_of!((*control_ptr).active)) != 0 }
}

fn read_control_config(control_ptr: *const VcamSharedFeedControl) -> VCAM_FEED_CONFIG {
    unsafe { read_volatile(addr_of!((*control_ptr).config)) }
}

fn read_control_slot_count(control_ptr: *const VcamSharedFeedControl) -> u32 {
    unsafe { read_volatile(addr_of!((*control_ptr).slot_count)) }
}

fn read_control_slot_bytes(control_ptr: *const VcamSharedFeedControl) -> u32 {
    unsafe { read_volatile(addr_of!((*control_ptr).slot_bytes)) }
}

fn read_control_committed_slot(control_ptr: *const VcamSharedFeedControl) -> u32 {
    unsafe { read_volatile(addr_of!((*control_ptr).committed_slot)) }
}

fn read_control_committed_frame_id(control_ptr: *const VcamSharedFeedControl) -> u64 {
    unsafe { read_volatile(addr_of!((*control_ptr).committed_frame_id)) }
}

fn read_control_last_timestamp(control_ptr: *const VcamSharedFeedControl) -> i64 {
    unsafe { read_volatile(addr_of!((*control_ptr).last_timestamp_100ns)) }
}

fn read_control_heartbeat_ms(control_ptr: *const VcamSharedFeedControl) -> u64 {
    let low = unsafe { read_volatile(addr_of!((*control_ptr).reserved0)) } as u64;
    let high = unsafe { read_volatile(addr_of!((*control_ptr).reserved1)) } as u64;
    (high << 32) | low
}

fn write_control_heartbeat_ms(control_ptr: *mut VcamSharedFeedControl, value: u64) {
    unsafe {
        write_volatile(addr_of_mut!((*control_ptr).reserved0), value as u32);
        write_volatile(addr_of_mut!((*control_ptr).reserved1), (value >> 32) as u32);
    }
}

fn deactivate_stale_session_locked(control_ptr: *mut VcamSharedFeedControl) {
    if !read_control_active_bit(control_ptr) || is_control_heartbeat_fresh(control_ptr) {
        return;
    }
    debug_log("shared feed heartbeat expired; deactivating stale session");
    unsafe {
        write_volatile(addr_of_mut!((*control_ptr).active), 0);
    }
}

fn is_control_heartbeat_fresh(control_ptr: *const VcamSharedFeedControl) -> bool {
    let heartbeat_ms = read_control_heartbeat_ms(control_ptr);
    if heartbeat_ms == 0 {
        return false;
    }

    let threshold_ms = feed_stale_threshold_ms(read_control_config(control_ptr));
    current_unix_time_ms().saturating_sub(heartbeat_ms) <= threshold_ms
}

fn feed_stale_threshold_ms(config: VCAM_FEED_CONFIG) -> u64 {
    if config.fps_num == 0 {
        return FEED_STALE_GRACE_MS_MIN;
    }
    let frame_ms = ((config.fps_den as u64)
        .saturating_mul(4_000)
        .saturating_add(config.fps_num as u64 - 1))
        / config.fps_num as u64;
    frame_ms.max(FEED_STALE_GRACE_MS_MIN)
}

fn current_unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn slot_region_bytes(payload_bytes: usize) -> Result<usize> {
    size_of::<VcamSharedFeedSlotHeader>()
        .checked_add(payload_bytes)
        .ok_or_else(|| Error::new(E_INVALIDARG.into(), "slot region byte count overflow"))
}

fn split_mapping_size(size: usize) -> (u32, u32) {
    let size = size as u64;
    ((size >> 32) as u32, size as u32)
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_null_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn session_keepalive() -> &'static Mutex<Option<SessionKeepAlive>> {
    static KEEPALIVE: OnceLock<Mutex<Option<SessionKeepAlive>>> = OnceLock::new();
    KEEPALIVE.get_or_init(|| Mutex::new(None))
}

fn ensure_shared_root_exists() -> Result<()> {
    let root = feed_shared_root_path();
    if root.is_dir() {
        return Ok(());
    }
    if root.exists() {
        return Err(Error::new(
            E_FAIL.into(),
            format!("shared feed root '{}' exists but is not a directory", root.display()),
        ));
    }

    let security = SharedSecurityAttributes::new()?;
    let root_w = wide_null_path(&root);
    match unsafe {
        CreateDirectoryW(
            PCWSTR(root_w.as_ptr()),
            Some(&security.attributes as *const SECURITY_ATTRIBUTES),
        )
    } {
        Ok(()) => Ok(()),
        Err(err) if err.code() == ERROR_ALREADY_EXISTS.to_hresult() => Ok(()),
        Err(err) => Err(err),
    }
}

fn open_backing_file(path: &Path, writable: bool, create: bool) -> Result<OwnedHandle> {
    try_open_backing_file_impl(path, writable, create)?
        .ok_or_else(|| Error::new(ERROR_FILE_NOT_FOUND.to_hresult(), "shared feed backing file does not exist"))
}

fn try_open_backing_file(path: &Path, writable: bool) -> Result<Option<OwnedHandle>> {
    try_open_backing_file_impl(path, writable, false)
}

fn try_open_backing_file_impl(path: &Path, writable: bool, create: bool) -> Result<Option<OwnedHandle>> {
    let desired_access = if writable {
        FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0
    } else {
        FILE_GENERIC_READ.0
    };
    let security = if create {
        Some(SharedSecurityAttributes::new()?)
    } else {
        None
    };
    let path_w = wide_null_path(path);
    let result = unsafe {
        CreateFileW(
            PCWSTR(path_w.as_ptr()),
            desired_access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            security
                .as_ref()
                .map(|security| &security.attributes as *const SECURITY_ATTRIBUTES),
            if create { OPEN_ALWAYS } else { OPEN_EXISTING },
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
        )
    };
    match result {
        Ok(handle) => Ok(Some(OwnedHandle(handle))),
        Err(err)
            if !create
                && (err.code() == ERROR_FILE_NOT_FOUND.to_hresult()
                    || err.code() == ERROR_PATH_NOT_FOUND.to_hresult()) =>
        {
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

fn current_file_size(handle: HANDLE) -> Result<u64> {
    let mut size = 0i64;
    unsafe {
        GetFileSizeEx(handle, &mut size)?;
    }
    Ok(size as u64)
}

fn ensure_file_size(handle: HANDLE, size: usize) -> Result<()> {
    if current_file_size(handle)? == size as u64 {
        return Ok(());
    }
    unsafe {
        SetFilePointerEx(handle, size as i64, None, FILE_BEGIN)?;
        SetEndOfFile(handle)?;
    }
    Ok(())
}

struct SharedSecurityAttributes {
    descriptor: PSECURITY_DESCRIPTOR,
    attributes: SECURITY_ATTRIBUTES,
}

impl SharedSecurityAttributes {
    fn new() -> Result<Self> {
        let sddl = wide_null("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;WD)(A;;GA;;;AC)");
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1 as u32,
                &mut descriptor,
                None,
            )?;
        }
        Ok(Self {
            descriptor,
            attributes: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0.cast(),
                bInheritHandle: BOOL(0),
            },
        })
    }
}

impl Drop for SharedSecurityAttributes {
    fn drop(&mut self) {
        if !self.descriptor.is_invalid() {
            unsafe {
                let _ = LocalFree(HLOCAL(self.descriptor.0.cast()));
            }
        }
    }
}
