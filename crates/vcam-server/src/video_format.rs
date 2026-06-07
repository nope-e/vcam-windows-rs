use windows::core::{Error, Result};
use windows::Win32::Foundation::E_INVALIDARG;

use crate::constants::{
    BGRA_BYTES_PER_PIXEL, DEFAULT_FRAME_HEIGHT, DEFAULT_FRAME_RATE_DENOMINATOR,
    DEFAULT_FRAME_RATE_NUMERATOR, DEFAULT_FRAME_WIDTH,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoFormat {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
}

impl Default for VideoFormat {
    fn default() -> Self {
        Self {
            width: DEFAULT_FRAME_WIDTH,
            height: DEFAULT_FRAME_HEIGHT,
            fps_num: DEFAULT_FRAME_RATE_NUMERATOR,
            fps_den: DEFAULT_FRAME_RATE_DENOMINATOR,
        }
    }
}

impl VideoFormat {
    pub fn new(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Result<Self> {
        let format = Self {
            width,
            height,
            fps_num,
            fps_den,
        };
        format.validate()?;
        Ok(format)
    }

    pub fn validate(self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(Error::new(E_INVALIDARG.into(), "frame dimensions must be non-zero"));
        }
        if self.fps_num == 0 || self.fps_den == 0 {
            return Err(Error::new(E_INVALIDARG.into(), "frame rate must be non-zero"));
        }
        if self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(Error::new(
                E_INVALIDARG.into(),
                "frame dimensions must be even for NV12 output",
            ));
        }
        Ok(())
    }

    pub fn frame_duration_100ns(self) -> i64 {
        ((10_000_000u64 * self.fps_den as u64) / self.fps_num as u64) as i64
    }

    pub fn bgra_stride(self) -> usize {
        self.width as usize * BGRA_BYTES_PER_PIXEL
    }

    pub fn bgra_frame_bytes(self) -> usize {
        self.bgra_stride() * self.height as usize
    }

    pub fn nv12_stride(self) -> usize {
        self.width as usize
    }

    pub fn nv12_frame_bytes(self) -> usize {
        self.width as usize * self.height as usize * 3 / 2
    }
}
