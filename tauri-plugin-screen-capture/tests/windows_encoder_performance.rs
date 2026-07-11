#![cfg(windows)]

use std::{sync::Arc, time::Instant};

use tauri_plugin_screen_capture::{
    pipeline::frame::VideoFrame, webrtc::h264_encoder::H264Encoder, PixelFormat,
};

#[test]
#[ignore = "CPU/GPU dependent Windows performance benchmark"]
fn h264_encoder_sustains_realtime_1080p60_input() {
    let width = 1920;
    let height = 1080;
    let fps = 60;
    let mut encoder = H264Encoder::new(width, height, fps).expect("create Windows H264 encoder");
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
        pixel[0] = (index % 251) as u8;
        pixel[1] = (index / 251 % 241) as u8;
        pixel[2] = (index / (251 * 241) % 239) as u8;
        pixel[3] = 255;
    }

    let started = Instant::now();
    let frame_count = fps * 3;
    let mut encoded = 0usize;
    for index in 0..frame_count {
        pixels[0] = index as u8;
        let frame = VideoFrame {
            width,
            height,
            pixel_format: PixelFormat::Bgra,
            timestamp_ns: u64::from(index) * 1_000_000_000 / u64::from(fps),
            data: Arc::from(pixels.clone()),
        };
        if encoder
            .encode_frame(&frame)
            .expect("encode Windows frame")
            .is_some()
        {
            encoded += 1;
        }
    }
    let elapsed = started.elapsed();
    let input_fps = f64::from(frame_count) / elapsed.as_secs_f64();
    eprintln!(
        "WINDOWS_ENCODER_BASELINE frames={} encoded={} input_fps={:.2} elapsed_ms={:.2}",
        frame_count,
        encoded,
        input_fps,
        elapsed.as_secs_f64() * 1000.0,
    );

    assert!(
        input_fps >= 55.0,
        "expected encoder input throughput >= 55 FPS, got {input_fps:.2}"
    );
    assert!(
        encoded * 100 >= frame_count as usize * 95,
        "expected at least 95% of input frames to produce H264 access units, got {encoded}/{frame_count}"
    );
}
