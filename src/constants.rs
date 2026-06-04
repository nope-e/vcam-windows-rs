use windows::core::GUID;

pub const ACTIVATE_CLSID: GUID = GUID::from_u128(0x44e66ac5_04be_4e2f_9d15_fea1430df6b5);
pub const ACTIVATE_CLSID_STRING: &str = "{44E66AC5-04BE-4E2F-9D15-FEA1430DF6B5}";

pub const FRIENDLY_NAME: &str = "Rust StaticCam Prototype";
pub const STREAM_ID: u32 = 0;

pub const FRAME_WIDTH: u32 = 640;
pub const FRAME_HEIGHT: u32 = 480;
pub const FRAME_RATE_NUMERATOR: u32 = 30;
pub const FRAME_RATE_DENOMINATOR: u32 = 1;
pub const FRAME_DURATION_100NS: i64 = 10_000_000i64 / FRAME_RATE_NUMERATOR as i64;

pub const BGRA_BYTES_PER_PIXEL: usize = 4;
pub const BGRA_FRAME_BYTES: usize =
    FRAME_WIDTH as usize * FRAME_HEIGHT as usize * BGRA_BYTES_PER_PIXEL;
pub const NV12_FRAME_BYTES: usize = FRAME_WIDTH as usize * FRAME_HEIGHT as usize * 3 / 2;
