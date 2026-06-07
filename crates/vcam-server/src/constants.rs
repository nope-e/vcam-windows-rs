use windows::core::GUID;

pub const ACTIVATE_CLSID: GUID = GUID::from_u128(0x44e66ac5_04be_4e2f_9d15_fea1430df6b5);
pub const ACTIVATE_CLSID_STRING: &str = "{44E66AC5-04BE-4E2F-9D15-FEA1430DF6B5}";

pub const FRAME_BROKER_CLSID: GUID = GUID::from_u128(0x8ec7c5b8_2c80_42f3_950d_70d4a8fb564e);
pub const FRAME_BROKER_CLSID_STRING: &str = "{8EC7C5B8-2C80-42F3-950D-70D4A8FB564E}";

pub const FRIENDLY_NAME: &str = "Rust StaticCam Prototype";
pub const FRAME_BROKER_NAME: &str = "Rust VCam Frame Broker";
pub const STREAM_ID: u32 = 0;

pub const DEFAULT_FRAME_WIDTH: u32 = 1920;
pub const DEFAULT_FRAME_HEIGHT: u32 = 1080;
pub const DEFAULT_FRAME_RATE_NUMERATOR: u32 = 60;
pub const DEFAULT_FRAME_RATE_DENOMINATOR: u32 = 1;

pub const BGRA_BYTES_PER_PIXEL: usize = 4;
pub const VCAM_FEED_INPUT_FORMAT_BGRA8: u32 = 1;
pub const VCAM_FEED_SLOT_COUNT: u32 = 3;
pub const VCAM_FEED_MAGIC: u32 = 0x5643_414d;
pub const VCAM_FEED_VERSION: u32 = 1;
pub const VCAM_INVALID_SLOT_INDEX: u32 = u32::MAX;
