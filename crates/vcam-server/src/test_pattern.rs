use std::path::Path;
use std::ptr::copy_nonoverlapping;

use windows::core::{Error, Result, HRESULT};
use windows::Win32::Foundation::E_INVALIDARG;

use crate::constants::{
    BGRA_FRAME_BYTES, BGRA_BYTES_PER_PIXEL, FRAME_HEIGHT, FRAME_WIDTH, NV12_FRAME_BYTES,
};

#[derive(Clone)]
pub struct StaticTestPattern {
    bgra: Vec<u8>,
    nv12: Vec<u8>,
}

impl StaticTestPattern {
    pub fn new() -> Self {
        let mut bgra = vec![0u8; BGRA_FRAME_BYTES];
        fill_color_bars(&mut bgra);
        overlay_guides(&mut bgra);
        let nv12 = bgra_to_nv12(&bgra);
        Self { bgra, nv12 }
    }

    pub fn rgb32_bytes(&self) -> &[u8] {
        &self.bgra
    }

    pub fn nv12_bytes(&self) -> &[u8] {
        &self.nv12
    }

    pub fn copy_to_rgb32_surface(
        &self,
        scanline0: *mut u8,
        pitch: i32,
        buffer_len: u32,
    ) -> Result<()> {
        copy_rows_to_surface(
            &self.bgra,
            FRAME_WIDTH as usize * BGRA_BYTES_PER_PIXEL,
            scanline0,
            pitch,
            FRAME_WIDTH as usize * BGRA_BYTES_PER_PIXEL,
            FRAME_HEIGHT as usize,
            buffer_len as usize,
        )
    }

    pub fn copy_to_nv12_surface(
        &self,
        scanline0: *mut u8,
        pitch: i32,
        buffer_start: *mut u8,
        buffer_len: u32,
    ) -> Result<()> {
        if pitch <= 0 {
            return Err(Error::new(
                E_INVALIDARG.into(),
                "NV12 surface pitch must be positive",
            ));
        }
        if scanline0.is_null() || buffer_start.is_null() {
            return Err(Error::new(E_INVALIDARG.into(), "NV12 surface pointer is null"));
        }

        let pitch = pitch as usize;
        let width = FRAME_WIDTH as usize;
        let height = FRAME_HEIGHT as usize;
        let y_plane_bytes = pitch * height;
        let uv_plane_bytes = pitch * (height / 2);
        let required_len = y_plane_bytes + uv_plane_bytes;
        if (buffer_len as usize) < required_len {
            return Err(Error::new(
                E_INVALIDARG.into(),
                "NV12 surface buffer is smaller than expected",
            ));
        }

        let (y_plane, uv_plane) = self.nv12.split_at(width * height);
        copy_rows_to_surface(
            y_plane,
            width,
            scanline0,
            pitch as i32,
            width,
            height,
            y_plane_bytes,
        )?;

        let uv_start = unsafe { buffer_start.add(y_plane_bytes) };
        copy_rows_to_surface(
            uv_plane,
            width,
            uv_start,
            pitch as i32,
            width,
            height / 2,
            uv_plane_bytes,
        )
    }

    pub fn write_bmp(&self, path: &Path) -> Result<()> {
        write_bytes(path, &self.to_bmp_bytes())?;
        Ok(())
    }

    fn to_bmp_bytes(&self) -> Vec<u8> {
        let file_header_size = 14u32;
        let dib_header_size = 40u32;
        let pixel_offset = file_header_size + dib_header_size;
        let file_size = pixel_offset + self.bgra.len() as u32;

        let mut out = Vec::with_capacity(file_size as usize);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&file_size.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&pixel_offset.to_le_bytes());

        out.extend_from_slice(&dib_header_size.to_le_bytes());
        out.extend_from_slice(&(FRAME_WIDTH as i32).to_le_bytes());
        out.extend_from_slice(&(FRAME_HEIGHT as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(self.bgra.len() as u32).to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());

        for row in (0..FRAME_HEIGHT as usize).rev() {
            let start = row * FRAME_WIDTH as usize * BGRA_BYTES_PER_PIXEL;
            let end = start + FRAME_WIDTH as usize * BGRA_BYTES_PER_PIXEL;
            out.extend_from_slice(&self.bgra[start..end]);
        }

        out
    }
}

pub fn write_bgra_bmp(path: &Path, bgra: &[u8]) -> Result<()> {
    if bgra.len() != BGRA_FRAME_BYTES {
        return Err(Error::new(
            E_INVALIDARG.into(),
            format!(
                "RGB32 frame size mismatch: expected {BGRA_FRAME_BYTES} bytes, got {}",
                bgra.len()
            ),
        ));
    }

    write_bytes(path, &bgra_to_bmp_bytes(bgra))
}

pub fn write_nv12_bmp(path: &Path, nv12: &[u8]) -> Result<()> {
    let bgra = nv12_to_bgra_bytes(nv12)?;
    write_bytes(path, &bgra_to_bmp_bytes(&bgra))
}

fn fill_color_bars(buffer: &mut [u8]) {
    let colors = [
        [255, 255, 255, 255],
        [0, 255, 255, 255],
        [255, 255, 0, 255],
        [0, 255, 0, 255],
        [255, 0, 255, 255],
        [0, 0, 255, 255],
        [255, 0, 0, 255],
        [0, 0, 0, 255],
    ];
    let width = FRAME_WIDTH as usize;
    let height = FRAME_HEIGHT as usize;
    let top_rows = height * 2 / 3;
    let bar_width = width / colors.len();

    for y in 0..height {
        for x in 0..width {
            let pixel = (y * width + x) * BGRA_BYTES_PER_PIXEL;
            let color = if y < top_rows {
                colors[(x / bar_width).min(colors.len() - 1)]
            } else {
                let intensity = (((x as f32 / width as f32) * 255.0) as u8).saturating_sub(16);
                [intensity, intensity, intensity, 255]
            };
            buffer[pixel..pixel + 4].copy_from_slice(&color);
        }
    }
}

fn overlay_guides(buffer: &mut [u8]) {
    let width = FRAME_WIDTH as usize;
    let height = FRAME_HEIGHT as usize;
    let center_x = width / 2;
    let center_y = height / 2;

    for y in 0..height {
        set_pixel(buffer, center_x, y, [32, 32, 32, 255]);
        let diag_x = y * width / height;
        set_pixel(buffer, diag_x, y, [32, 32, 32, 255]);
        set_pixel(buffer, width - 1 - diag_x, y, [32, 32, 32, 255]);
    }

    for x in 0..width {
        set_pixel(buffer, x, center_y, [32, 32, 32, 255]);
    }

    let box_w = width / 5;
    let box_h = height / 8;
    let start_x = width / 2 - box_w / 2;
    let start_y = height - box_h - 20;
    for y in start_y..start_y + box_h {
        for x in start_x..start_x + box_w {
            let border = x == start_x
                || x == start_x + box_w - 1
                || y == start_y
                || y == start_y + box_h - 1;
            let color = if border {
                [255, 255, 255, 255]
            } else {
                [24, 24, 24, 255]
            };
            set_pixel(buffer, x, y, color);
        }
    }
}

fn set_pixel(buffer: &mut [u8], x: usize, y: usize, bgra: [u8; 4]) {
    let width = FRAME_WIDTH as usize;
    let height = FRAME_HEIGHT as usize;
    if x >= width || y >= height {
        return;
    }
    let offset = (y * width + x) * BGRA_BYTES_PER_PIXEL;
    buffer[offset..offset + 4].copy_from_slice(&bgra);
}

fn bgra_to_nv12(bgra: &[u8]) -> Vec<u8> {
    let width = FRAME_WIDTH as usize;
    let height = FRAME_HEIGHT as usize;
    let mut out = vec![0u8; NV12_FRAME_BYTES];
    let (y_plane, uv_plane) = out.split_at_mut(width * height);

    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * BGRA_BYTES_PER_PIXEL;
            let b = bgra[offset] as f32;
            let g = bgra[offset + 1] as f32;
            let r = bgra[offset + 2] as f32;
            y_plane[y * width + x] = clamp_u8(0.257 * r + 0.504 * g + 0.098 * b + 16.0);
        }
    }

    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let mut u_sum = 0.0f32;
            let mut v_sum = 0.0f32;
            for sample_y in y..(y + 2) {
                for sample_x in x..(x + 2) {
                    let offset = (sample_y * width + sample_x) * BGRA_BYTES_PER_PIXEL;
                    let b = bgra[offset] as f32;
                    let g = bgra[offset + 1] as f32;
                    let r = bgra[offset + 2] as f32;
                    u_sum += -0.148 * r - 0.291 * g + 0.439 * b + 128.0;
                    v_sum += 0.439 * r - 0.368 * g - 0.071 * b + 128.0;
                }
            }

            let uv_offset = (y / 2) * width + x;
            uv_plane[uv_offset] = clamp_u8(u_sum / 4.0);
            uv_plane[uv_offset + 1] = clamp_u8(v_sum / 4.0);
        }
    }

    out
}

fn clamp_u8(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

fn nv12_to_bgra_bytes(nv12: &[u8]) -> Result<Vec<u8>> {
    if nv12.len() != NV12_FRAME_BYTES {
        return Err(Error::new(
            E_INVALIDARG.into(),
            format!(
                "NV12 frame size mismatch: expected {NV12_FRAME_BYTES} bytes, got {}",
                nv12.len()
            ),
        ));
    }

    let width = FRAME_WIDTH as usize;
    let height = FRAME_HEIGHT as usize;
    let (y_plane, uv_plane) = nv12.split_at(width * height);
    let mut out = vec![0u8; BGRA_FRAME_BYTES];

    for y in 0..height {
        for x in 0..width {
            let luma = y_plane[y * width + x] as f32;
            let uv_offset = (y / 2) * width + (x & !1);
            let u = uv_plane[uv_offset] as f32;
            let v = uv_plane[uv_offset + 1] as f32;

            let c = (luma - 16.0).max(0.0);
            let d = u - 128.0;
            let e = v - 128.0;
            let b = clamp_u8(1.164 * c + 2.017 * d);
            let g = clamp_u8(1.164 * c - 0.392 * d - 0.813 * e);
            let r = clamp_u8(1.164 * c + 1.596 * e);

            let pixel = (y * width + x) * BGRA_BYTES_PER_PIXEL;
            out[pixel..pixel + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }

    Ok(out)
}

pub fn validate_dump_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(Error::new(E_INVALIDARG.into(), "dump path is empty"));
    }
    Ok(())
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)
        .map_err(|err| Error::new(HRESULT(0x80070020u32 as i32), format!("{err}")))?;
    Ok(())
}

fn bgra_to_bmp_bytes(bgra: &[u8]) -> Vec<u8> {
    let file_header_size = 14u32;
    let dib_header_size = 40u32;
    let pixel_offset = file_header_size + dib_header_size;
    let file_size = pixel_offset + bgra.len() as u32;

    let mut out = Vec::with_capacity(file_size as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&pixel_offset.to_le_bytes());

    out.extend_from_slice(&dib_header_size.to_le_bytes());
    out.extend_from_slice(&(FRAME_WIDTH as i32).to_le_bytes());
    out.extend_from_slice(&(FRAME_HEIGHT as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(bgra.len() as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for row in (0..FRAME_HEIGHT as usize).rev() {
        let start = row * FRAME_WIDTH as usize * BGRA_BYTES_PER_PIXEL;
        let end = start + FRAME_WIDTH as usize * BGRA_BYTES_PER_PIXEL;
        out.extend_from_slice(&bgra[start..end]);
    }

    out
}

fn copy_rows_to_surface(
    src: &[u8],
    src_stride: usize,
    dst_start: *mut u8,
    dst_pitch: i32,
    row_bytes: usize,
    rows: usize,
    dst_len: usize,
) -> Result<()> {
    if dst_start.is_null() {
        return Err(Error::new(E_INVALIDARG.into(), "surface pointer is null"));
    }
    if dst_pitch == 0 {
        return Err(Error::new(E_INVALIDARG.into(), "surface pitch must not be zero"));
    }

    let abs_pitch = dst_pitch.unsigned_abs() as usize;
    if abs_pitch < row_bytes {
        return Err(Error::new(
            E_INVALIDARG.into(),
            "surface pitch is smaller than a frame row",
        ));
    }

    let required_len = abs_pitch
        .saturating_mul(rows.saturating_sub(1))
        .saturating_add(row_bytes);
    if dst_len < required_len {
        return Err(Error::new(
            E_INVALIDARG.into(),
            "surface buffer is smaller than expected",
        ));
    }

    let mut dst_row = dst_start;
    for row in 0..rows {
        let src_offset = row * src_stride;
        unsafe {
            copy_nonoverlapping(src.as_ptr().add(src_offset), dst_row, row_bytes);
            dst_row = dst_row.offset(dst_pitch as isize);
        }
    }

    Ok(())
}
