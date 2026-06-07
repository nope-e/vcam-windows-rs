use std::env;
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{Error, Result};
use windows::Win32::Foundation::{BOOL, E_FAIL};

use vcam_server::{create_frame_broker, FeedSessionProducer, VCAM_FEED_CONFIG, VideoFormat};

fn main() -> Result<()> {
    let _runtime = com::runtime::init_runtime()
        .map_err(|err| Error::new(E_FAIL.into(), format!("COM runtime init failed: {err:?}")))?;

    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("stream-animated") => stream_animated(parse_stream_options(&args[1..])?),
        _ => {
            print_usage();
            Ok(())
        }
    }
}

struct StreamOptions {
    width: u32,
    height: u32,
    fps: u32,
    duration_seconds: u32,
    force_reset: bool,
}

fn parse_stream_options(args: &[String]) -> Result<StreamOptions> {
    let defaults = VideoFormat::default();
    let mut options = StreamOptions {
        width: defaults.width,
        height: defaults.height,
        fps: defaults.fps_num,
        duration_seconds: 10,
        force_reset: false,
    };
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--width" => {
                options.width = parse_u32_arg(args, index + 1, "--width")?;
                index += 2;
            }
            "--height" => {
                options.height = parse_u32_arg(args, index + 1, "--height")?;
                index += 2;
            }
            "--fps" => {
                options.fps = parse_u32_arg(args, index + 1, "--fps")?;
                index += 2;
            }
            "--duration-seconds" => {
                options.duration_seconds = parse_u32_arg(args, index + 1, "--duration-seconds")?;
                index += 2;
            }
            "--force-reset" => {
                options.force_reset = true;
                index += 1;
            }
            other => {
                return Err(Error::new(
                    E_FAIL.into(),
                    format!("unknown option for stream-animated: {other}"),
                ));
            }
        }
    }

    Ok(options)
}

fn stream_animated(options: StreamOptions) -> Result<()> {
    let format = VideoFormat::new(options.width, options.height, options.fps, 1)?;
    let config = VCAM_FEED_CONFIG::from_video_format(format);
    let broker = create_frame_broker()?;

    unsafe {
        broker.StartSession(&config, BOOL(options.force_reset as i32))?;
    }

    let stop_result = (|| -> Result<()> {
        let mut producer = FeedSessionProducer::open(config)?;
        let duration = Duration::from_secs(options.duration_seconds as u64);
        let frame_period = Duration::from_secs_f64(1.0 / options.fps as f64);
        let start = Instant::now();
        let mut frame_id = 0u64;

        while start.elapsed() < duration {
            let frame = render_animated_frame(format, frame_id);
            let timestamp_100ns = frame_id as i64 * format.frame_duration_100ns();
            producer.publish_bgra_frame(frame_id, timestamp_100ns, &frame)?;
            frame_id += 1;

            let next_deadline = start + frame_period.mul_f64(frame_id as f64);
            sleep_until(next_deadline);
        }

        println!(
            "published {frame_id} frames at {}x{} {}fps",
            format.width, format.height, format.fps_num
        );
        Ok(())
    })();

    unsafe {
        let _ = broker.StopSession();
    }
    stop_result
}

fn render_animated_frame(format: VideoFormat, frame_id: u64) -> Vec<u8> {
    let width = format.width as usize;
    let height = format.height as usize;
    let stride = format.bgra_stride();
    let mut frame = vec![0u8; format.bgra_frame_bytes()];

    for y in 0..height {
        for x in 0..width {
            let offset = y * stride + x * 4;
            let wave = ((frame_id.wrapping_mul(3) as usize + x / 8 + y / 4) & 0xff) as u8;
            frame[offset] = ((x * 255) / width.max(1)) as u8;
            frame[offset + 1] = ((y * 255) / height.max(1)) as u8;
            frame[offset + 2] = wave;
            frame[offset + 3] = 255;
        }
    }

    let box_w = (width / 6).max(32).min(width);
    let box_h = (height / 6).max(32).min(height);
    let travel_x = width.saturating_sub(box_w).max(1);
    let travel_y = height.saturating_sub(box_h).max(1);
    let box_x = ((frame_id * 7) as usize) % travel_x;
    let box_y = ((frame_id * 5) as usize) % travel_y;
    draw_rect(&mut frame, stride, box_x, box_y, box_w, box_h, [255, 255, 255, 255], true);
    draw_rect(
        &mut frame,
        stride,
        box_x.saturating_add(4),
        box_y.saturating_add(4),
        box_w.saturating_sub(8),
        box_h.saturating_sub(8),
        [32, 32, 32, 255],
        false,
    );

    let cross_x = width / 2;
    let cross_y = height / 2;
    for y in 0..height {
        set_pixel(&mut frame, stride, cross_x, y, [16, 16, 16, 255]);
    }
    for x in 0..width {
        set_pixel(&mut frame, stride, x, cross_y, [16, 16, 16, 255]);
    }

    draw_counter(&mut frame, stride, width, height, frame_id);
    frame
}

fn draw_counter(frame: &mut [u8], stride: usize, width: usize, height: usize, frame_id: u64) {
    let text = frame_id.to_string();
    let glyph_w = 4usize;
    let glyph_h = 6usize;
    let scale = (height / 180).max(2);
    let total_w = text.len() * glyph_w * scale;
    let start_x = width.saturating_sub(total_w + 16);
    let start_y = height.saturating_sub(glyph_h * scale + 16);

    for (index, ch) in text.chars().enumerate() {
        if let Some(pattern) = digit_pattern(ch) {
            draw_digit(
                frame,
                stride,
                start_x + index * glyph_w * scale,
                start_y,
                scale,
                pattern,
            );
        }
    }
}

fn draw_digit(
    frame: &mut [u8],
    stride: usize,
    x: usize,
    y: usize,
    scale: usize,
    pattern: [u8; 5],
) {
    for (row, bits) in pattern.into_iter().enumerate() {
        for col in 0..3usize {
            if bits & (1 << (2 - col)) == 0 {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    set_pixel(
                        frame,
                        stride,
                        x + col * scale + dx,
                        y + row * scale + dy,
                        [255, 255, 255, 255],
                    );
                }
            }
        }
    }
}

fn digit_pattern(ch: char) -> Option<[u8; 5]> {
    match ch {
        '0' => Some([0b111, 0b101, 0b101, 0b101, 0b111]),
        '1' => Some([0b010, 0b110, 0b010, 0b010, 0b111]),
        '2' => Some([0b111, 0b001, 0b111, 0b100, 0b111]),
        '3' => Some([0b111, 0b001, 0b111, 0b001, 0b111]),
        '4' => Some([0b101, 0b101, 0b111, 0b001, 0b001]),
        '5' => Some([0b111, 0b100, 0b111, 0b001, 0b111]),
        '6' => Some([0b111, 0b100, 0b111, 0b101, 0b111]),
        '7' => Some([0b111, 0b001, 0b010, 0b010, 0b010]),
        '8' => Some([0b111, 0b101, 0b111, 0b101, 0b111]),
        '9' => Some([0b111, 0b101, 0b111, 0b001, 0b111]),
        _ => None,
    }
}

fn draw_rect(
    frame: &mut [u8],
    stride: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: [u8; 4],
    border_only: bool,
) {
    for row in 0..height {
        for col in 0..width {
            if border_only
                && row != 0
                && row != height.saturating_sub(1)
                && col != 0
                && col != width.saturating_sub(1)
            {
                continue;
            }
            set_pixel(frame, stride, x + col, y + row, color);
        }
    }
}

fn set_pixel(frame: &mut [u8], stride: usize, x: usize, y: usize, color: [u8; 4]) {
    let width = stride / 4;
    let height = frame.len() / stride;
    if x >= width || y >= height {
        return;
    }
    let offset = y * stride + x * 4;
    frame[offset..offset + 4].copy_from_slice(&color);
}

fn sleep_until(deadline: Instant) {
    let now = Instant::now();
    if deadline > now {
        thread::sleep(deadline - now);
    }
}

fn parse_u32_arg(args: &[String], index: usize, name: &str) -> Result<u32> {
    let value = args
        .get(index)
        .ok_or_else(|| Error::new(E_FAIL.into(), format!("missing value for {name}")))?;
    value
        .parse::<u32>()
        .map_err(|err| Error::new(E_FAIL.into(), format!("invalid value for {name}: {err}")))
}

fn print_usage() {
    println!("Usage:");
    println!(
        "  cargo run -p vcamfeed-demo -- stream-animated [--width <px>] [--height <px>] [--fps <n>] [--duration-seconds <n>] [--force-reset]"
    );
}
