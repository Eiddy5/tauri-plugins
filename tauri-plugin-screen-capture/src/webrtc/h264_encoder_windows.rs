use std::{
    io::{Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use openh264::{
    encoder::{Encoder, EncoderConfig, RateControlMode},
    formats::YUVBuffer,
    Timestamp,
};

use crate::{
    error::Error,
    models::{CaptureErrorCode, PixelFormat},
    pipeline::frame::VideoFrame,
    webrtc::h264_annexb::take_next_aud_access_unit,
    webrtc::track::EncodedVideoSample,
    Result,
};
use yuvutils_rs::{bgra_to_yuv420, YuvRange, YuvStandardMatrix};

const MAX_PENDING_H264_BYTES: usize = 8 * 1024 * 1024;

pub struct H264Encoder {
    backend: EncoderBackend,
    width: u32,
    height: u32,
    fps: u32,
}

unsafe impl Send for H264Encoder {}

enum EncoderBackend {
    FfmpegHardware(FfmpegHardwareH264Encoder),
    OpenH264(SoftwareH264Encoder),
}

impl H264Encoder {
    pub fn new(width: u32, height: u32, fps: u32) -> Result<Self> {
        match FfmpegHardwareH264Encoder::new(width, height, fps) {
            Ok(encoder) => {
                eprintln!("[screen-capture] Windows H264 encoder: FFmpeg hardware encoder");
                Ok(Self {
                    backend: EncoderBackend::FfmpegHardware(encoder),
                    width,
                    height,
                    fps: fps.max(1),
                })
            }
            Err(error) => {
                let message = error.payload().message;
                eprintln!("[screen-capture] Windows FFmpeg hardware H264 encoder unavailable, falling back to OpenH264: {message}");
                Ok(Self {
                    backend: EncoderBackend::OpenH264(SoftwareH264Encoder::new(
                        width, height, fps,
                    )?),
                    width,
                    height,
                    fps: fps.max(1),
                })
            }
        }
    }

    pub fn force_keyframe(&mut self) -> Result<()> {
        match &mut self.backend {
            EncoderBackend::FfmpegHardware(encoder) => encoder.force_keyframe(),
            EncoderBackend::OpenH264(encoder) => encoder.force_keyframe(),
        }
    }

    pub fn encode_frame(&mut self, frame: &VideoFrame) -> Result<Option<EncodedVideoSample>> {
        match &mut self.backend {
            EncoderBackend::FfmpegHardware(encoder) => encoder.encode_frame(frame),
            EncoderBackend::OpenH264(encoder) => encoder.encode_frame(frame),
        }
        .or_else(|error| {
            if !matches!(self.backend, EncoderBackend::OpenH264(_)) {
                let message = error.payload().message;
                eprintln!(
                    "[screen-capture] Windows hardware H264 encoder failed while publishing, falling back to OpenH264: {}",
                    message
                );
                self.backend =
                    EncoderBackend::OpenH264(SoftwareH264Encoder::new(self.width, self.height, self.fps)?);
                if let EncoderBackend::OpenH264(encoder) = &mut self.backend {
                    encoder.encode_frame(frame)
                } else {
                    Ok(None)
                }
            } else {
                Err(error)
            }
        })
    }
}

struct FfmpegHardwareH264Encoder {
    ffmpeg: String,
    encoder_kind: HardwareH264EncoderKind,
    child: Child,
    stdin: ChildStdin,
    stdout_rx: Receiver<Vec<u8>>,
    pending_h264: Vec<u8>,
    width: u32,
    height: u32,
    fps: u32,
    frame_duration: Duration,
    pending_force_keyframe: bool,
}

impl FfmpegHardwareH264Encoder {
    fn new(width: u32, height: u32, fps: u32) -> Result<Self> {
        validate_bgra_encoder_input(width, height, "FFmpeg hardware")?;
        let ffmpeg = locate_ffmpeg()?;
        let available = ffmpeg_encoders(&ffmpeg)?;
        for encoder_kind in HardwareH264EncoderKind::preferred_order() {
            if !available.contains(encoder_kind.ffmpeg_name()) {
                continue;
            }
            match probe_ffmpeg_encoder(&ffmpeg, encoder_kind, width, height, fps) {
                Ok(()) => {
                    let encoder_name = encoder_kind.ffmpeg_name();
                    eprintln!(
                        "[screen-capture] Windows FFmpeg hardware H264 encoder selected: {encoder_name}"
                    );
                    return Self::spawn(ffmpeg, encoder_kind, width, height, fps);
                }
                Err(error) => {
                    let message = error.payload().message;
                    let encoder_name = encoder_kind.ffmpeg_name();
                    eprintln!(
                        "[screen-capture] Windows FFmpeg hardware H264 encoder probe failed for {encoder_name}: {message}"
                    );
                }
            }
        }
        Err(Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            "No usable FFmpeg hardware H264 encoder found. Expected one of h264_nvenc, h264_qsv, h264_amf.",
            true,
        ))
    }

    fn spawn(
        ffmpeg: String,
        encoder_kind: HardwareH264EncoderKind,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self> {
        let (child, stdin, stdout_rx) =
            spawn_ffmpeg_process(&ffmpeg, encoder_kind, width, height, fps)?;
        Ok(Self {
            ffmpeg,
            encoder_kind,
            child,
            stdin,
            stdout_rx,
            pending_h264: Vec::new(),
            width,
            height,
            fps: fps.max(1),
            frame_duration: Duration::from_secs_f64(1.0 / f64::from(fps.max(1))),
            pending_force_keyframe: false,
        })
    }

    fn force_keyframe(&mut self) -> Result<()> {
        self.pending_force_keyframe = true;
        Ok(())
    }

    fn restart_encoder(&mut self) -> Result<()> {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let (child, stdin, stdout_rx) = spawn_ffmpeg_process(
            &self.ffmpeg,
            self.encoder_kind,
            self.width,
            self.height,
            self.fps,
        )?;
        self.child = child;
        self.stdin = stdin;
        self.stdout_rx = stdout_rx;
        self.pending_h264.clear();
        self.pending_force_keyframe = false;
        Ok(())
    }

    fn encode_frame(&mut self, frame: &VideoFrame) -> Result<Option<EncodedVideoSample>> {
        if frame.pixel_format != PixelFormat::Bgra {
            return Err(Error::new(
                CaptureErrorCode::WebRtcTrackFailed,
                "Windows FFmpeg hardware encoder expects BGRA frames",
                true,
            ));
        }
        if frame.width != self.width || frame.height != self.height {
            return Err(Error::new(
                CaptureErrorCode::WebRtcTrackFailed,
                format!(
                    "Windows FFmpeg hardware encoder frame size changed from {}x{} to {}x{}",
                    self.width, self.height, frame.width, frame.height
            ),
                true,
            ));
        }
        if self.pending_force_keyframe {
            self.restart_encoder()?;
        }
        self.stdin.write_all(&frame.data).map_err(ffmpeg_error)?;
        self.stdin.flush().map_err(ffmpeg_error)?;

        while let Ok(chunk) = self.stdout_rx.try_recv() {
            self.pending_h264.extend_from_slice(&chunk);
            if self.pending_h264.len() > MAX_PENDING_H264_BYTES {
                return Err(Error::new(
                    CaptureErrorCode::WebRtcTrackFailed,
                    format!(
                        "Windows FFmpeg hardware encoder output did not contain H264 access-unit boundaries after {} bytes",
                        self.pending_h264.len()
                    ),
                    true,
                ));
            }
        }
        let Some(data) = take_next_aud_access_unit(&mut self.pending_h264) else {
            return Ok(None);
        };

        Ok(Some(EncodedVideoSample {
            data,
            duration: self.frame_duration,
            timestamp_ns: frame.timestamp_ns,
        }))
    }
}

impl Drop for FfmpegHardwareH264Encoder {
    fn drop(&mut self) {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
        eprintln!(
            "[screen-capture] Windows FFmpeg hardware H264 encoder stopped: {}",
            self.encoder_kind.ffmpeg_name()
        );
    }
}

#[derive(Clone, Copy)]
enum HardwareH264EncoderKind {
    Nvenc,
    Qsv,
    Amf,
}

impl HardwareH264EncoderKind {
    fn preferred_order() -> [Self; 3] {
        if let Ok(choice) = std::env::var("TAURI_PLUGIN_SCREEN_CAPTURE_WINDOWS_H264_ENCODER") {
            let choice = choice.trim().to_ascii_lowercase();
            let first = match choice.as_str() {
                "amf" => Some(Self::Amf),
                "qsv" => Some(Self::Qsv),
                "nvenc" => Some(Self::Nvenc),
                _ => None,
            };
            if let Some(first) = first {
                return match first {
                    Self::Amf => [Self::Amf, Self::Qsv, Self::Nvenc],
                    Self::Qsv => [Self::Qsv, Self::Amf, Self::Nvenc],
                    Self::Nvenc => [Self::Nvenc, Self::Amf, Self::Qsv],
                };
            }
        }
        [Self::Amf, Self::Qsv, Self::Nvenc]
    }

    fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Nvenc => "h264_nvenc",
            Self::Qsv => "h264_qsv",
            Self::Amf => "h264_amf",
        }
    }
}

struct SoftwareH264Encoder {
    encoder: Encoder,
    width: u32,
    height: u32,
    frame_duration: Duration,
}

impl SoftwareH264Encoder {
    fn new(width: u32, height: u32, fps: u32) -> Result<Self> {
        validate_bgra_encoder_input(width, height, "OpenH264")?;
        let config = EncoderConfig::new()
            .set_bitrate_bps(recommended_bitrate(width, height, fps))
            .max_frame_rate(fps.max(1) as f32)
            .enable_skip_frame(true)
            .rate_control_mode(RateControlMode::Bitrate);
        let api = openh264::OpenH264API::from_source();
        let encoder = Encoder::with_api_config(api, config).map_err(encoder_error)?;
        Ok(Self {
            encoder,
            width,
            height,
            frame_duration: Duration::from_secs_f64(1.0 / f64::from(fps.max(1))),
        })
    }

    fn force_keyframe(&mut self) -> Result<()> {
        self.encoder.force_intra_frame();
        Ok(())
    }

    fn encode_frame(&mut self, frame: &VideoFrame) -> Result<Option<EncodedVideoSample>> {
        if frame.pixel_format != PixelFormat::Bgra {
            return Err(Error::new(
                CaptureErrorCode::WebRtcTrackFailed,
                "Windows OpenH264 encoder expects BGRA frames",
                true,
            ));
        }
        if frame.width != self.width || frame.height != self.height {
            return Err(Error::new(
                CaptureErrorCode::WebRtcTrackFailed,
                format!(
                    "Windows OpenH264 encoder frame size changed from {}x{} to {}x{}",
                    self.width, self.height, frame.width, frame.height
                ),
                true,
            ));
        }

        let width = frame.width as usize;
        let height = frame.height as usize;
        let y_len = width * height;
        let uv_len = y_len / 4;
        let mut yuv_data = vec![0u8; y_len + uv_len * 2];
        let (y_plane, uv_planes) = yuv_data.split_at_mut(y_len);
        let (u_plane, v_plane) = uv_planes.split_at_mut(uv_len);
        bgra_to_yuv420(
            y_plane,
            frame.width,
            u_plane,
            frame.width / 2,
            v_plane,
            frame.width / 2,
            &frame.data,
            frame.width * 4,
            frame.width,
            frame.height,
            YuvRange::TV,
            YuvStandardMatrix::Bt709,
        );
        let yuv = YUVBuffer::from_vec(yuv_data, width, height);
        let bitstream = self
            .encoder
            .encode_at(&yuv, Timestamp::from_millis(frame.timestamp_ns / 1_000_000))
            .map_err(encoder_error)?;
        let data = annex_b_from_bitstream(&bitstream);
        if data.is_empty() {
            return Ok(None);
        }

        Ok(Some(EncodedVideoSample {
            data,
            duration: self.frame_duration,
            timestamp_ns: frame.timestamp_ns,
        }))
    }
}

fn locate_ffmpeg() -> Result<String> {
    if let Ok(path) = std::env::var("TAURI_PLUGIN_SCREEN_CAPTURE_FFMPEG") {
        if !path.trim().is_empty() {
            return Ok(path);
        }
    }

    let candidates = ["ffmpeg.exe", "ffmpeg"];
    for candidate in candidates {
        if Command::new(candidate)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            return Ok(candidate.to_string());
        }
    }

    Err(Error::new(
        CaptureErrorCode::WebRtcTrackFailed,
        "FFmpeg was not found. Set TAURI_PLUGIN_SCREEN_CAPTURE_FFMPEG or bundle ffmpeg.exe with the app.",
        true,
    ))
}

fn ffmpeg_encoders(ffmpeg: &str) -> Result<String> {
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .map_err(ffmpeg_error)?;
    if !output.status.success() {
        return Err(Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            format!(
                "FFmpeg encoder listing failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
            true,
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn probe_ffmpeg_encoder(
    ffmpeg: &str,
    encoder_kind: HardwareH264EncoderKind,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<()> {
    let encoder_name = encoder_kind.ffmpeg_name();
    let mut child = Command::new(ffmpeg)
        .args(ffmpeg_probe_args(encoder_kind, width, height, fps))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(ffmpeg_error)?;
    let frame_size = width as usize * height as usize * 4;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&vec![0u8; frame_size]).map_err(ffmpeg_error)?;
    }
    let output = child.wait_with_output().map_err(ffmpeg_error)?;
    if output.status.success() && !output.stdout.is_empty() {
        Ok(())
    } else {
        Err(Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            format!(
                "FFmpeg encoder {encoder_name} failed probe: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
            true,
        ))
    }
}

fn ffmpeg_probe_args(
    encoder_kind: HardwareH264EncoderKind,
    width: u32,
    height: u32,
    fps: u32,
) -> Vec<String> {
    let mut args = ffmpeg_input_args(width, height, fps);
    args.extend(["-frames:v".into(), "1".into()]);
    args.extend(ffmpeg_encoder_args(encoder_kind, width, height, fps));
    args.extend(["-f".into(), "h264".into(), "pipe:1".into()]);
    args
}

fn ffmpeg_realtime_args(
    encoder_kind: HardwareH264EncoderKind,
    width: u32,
    height: u32,
    fps: u32,
) -> Vec<String> {
    let mut args = ffmpeg_input_args(width, height, fps);
    args.extend(ffmpeg_encoder_args(encoder_kind, width, height, fps));
    args.extend(["-f".into(), "h264".into(), "pipe:1".into()]);
    args
}

fn spawn_ffmpeg_process(
    ffmpeg: &str,
    encoder_kind: HardwareH264EncoderKind,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(Child, ChildStdin, Receiver<Vec<u8>>)> {
    let mut command = Command::new(ffmpeg);
    command.args(ffmpeg_realtime_args(encoder_kind, width, height, fps));
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(ffmpeg_error)?;
    let stdin = child.stdin.take().ok_or_else(|| {
        Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            "FFmpeg hardware encoder stdin is unavailable",
            true,
        )
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            "FFmpeg hardware encoder stdout is unavailable",
            true,
        )
    })?;
    if let Some(stderr) = child.stderr.take() {
        spawn_ffmpeg_stderr_logger(stderr);
    }
    let stdout_rx = spawn_ffmpeg_stdout_reader(stdout);
    Ok((child, stdin, stdout_rx))
}

fn ffmpeg_input_args(width: u32, height: u32, fps: u32) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "warning".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "bgra".into(),
        "-s:v".into(),
        format!("{width}x{height}"),
        "-r".into(),
        fps.max(1).to_string(),
        "-i".into(),
        "pipe:0".into(),
        "-an".into(),
    ]
}

fn ffmpeg_encoder_args(
    encoder_kind: HardwareH264EncoderKind,
    width: u32,
    height: u32,
    fps: u32,
) -> Vec<String> {
    let bitrate = recommended_bitrate(width, height, fps).to_string();
    let encoder_name = encoder_kind.ffmpeg_name();
    let mut args = vec!["-c:v".into(), encoder_name.into()];
    match encoder_kind {
        HardwareH264EncoderKind::Amf => args.extend([
            "-usage".into(),
            "ultralowlatency".into(),
            "-quality".into(),
            "speed".into(),
            "-rc".into(),
            "cbr".into(),
            "-latency".into(),
            "1".into(),
            "-profile:v".into(),
            "constrained_baseline".into(),
            "-level:v".into(),
            "5.2".into(),
            "-coder".into(),
            "cavlc".into(),
            "-forced_idr".into(),
            "1".into(),
            "-aud".into(),
            "1".into(),
            "-header_spacing".into(),
            "1".into(),
            "-async_depth".into(),
            "1".into(),
        ]),
        HardwareH264EncoderKind::Qsv => args.extend([
            "-preset".into(),
            "veryfast".into(),
            "-profile:v".into(),
            "baseline".into(),
            "-forced_idr".into(),
            "1".into(),
        ]),
        HardwareH264EncoderKind::Nvenc => args.extend([
            "-preset".into(),
            "p1".into(),
            "-tune".into(),
            "ull".into(),
            "-rc".into(),
            "cbr".into(),
            "-profile:v".into(),
            "baseline".into(),
            "-forced-idr".into(),
            "1".into(),
            "-zerolatency".into(),
            "1".into(),
        ]),
    }
    args.extend([
        "-b:v".into(),
        bitrate.clone(),
        "-maxrate".into(),
        bitrate.clone(),
        "-bufsize".into(),
        (recommended_bitrate(width, height, fps).saturating_mul(2)).to_string(),
        "-g".into(),
        fps.max(1).to_string(),
        "-bf".into(),
        "0".into(),
        "-pix_fmt".into(),
        "nv12".into(),
        "-bsf:v".into(),
        "h264_metadata=aud=insert".into(),
    ]);
    args
}

fn spawn_ffmpeg_stdout_reader(mut stdout: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 64 * 1024];
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(len) => {
                    if tx.send(buffer[..len].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

fn spawn_ffmpeg_stderr_logger(mut stderr: impl Read + Send + 'static) {
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) => break,
                Ok(len) => {
                    let message = String::from_utf8_lossy(&buffer[..len]);
                    for line in message.lines().filter(|line| !line.trim().is_empty()) {
                        eprintln!("[screen-capture] FFmpeg encoder: {line}");
                    }
                }
                Err(_) => break,
            }
        }
    });
}

fn annex_b_from_bitstream(bitstream: &openh264::encoder::EncodedBitStream<'_>) -> Vec<u8> {
    let mut data = Vec::new();
    for layer_index in 0..bitstream.num_layers() {
        let Some(layer) = bitstream.layer(layer_index) else {
            continue;
        };
        for nal_index in 0..layer.nal_count() {
            let Some(nal) = layer.nal_unit(nal_index) else {
                continue;
            };
            append_annex_b_nal(&mut data, nal);
        }
    }
    data
}

fn append_annex_b_nal(output: &mut Vec<u8>, nal: &[u8]) {
    let payload = nal
        .strip_prefix(&[0, 0, 0, 1])
        .or_else(|| nal.strip_prefix(&[0, 0, 1]))
        .unwrap_or(nal);
    if payload.is_empty() {
        return;
    }
    output.extend_from_slice(&[0, 0, 0, 1]);
    output.extend_from_slice(payload);
}

fn validate_bgra_encoder_input(width: u32, height: u32, encoder_name: &str) -> Result<()> {
    if width % 2 != 0 || height % 2 != 0 {
        return Err(Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            format!(
                "Windows {encoder_name} encoder requires even dimensions, got {width}x{height}",
            ),
            true,
        ));
    }
    Ok(())
}

fn recommended_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    let fps = u64::from(fps.max(1));
    let bits = pixels
        .saturating_mul(fps)
        .saturating_mul(8)
        .saturating_div(100);
    u32::try_from(bits.clamp(6_000_000, 24_000_000)).unwrap_or(12_000_000)
}

fn encoder_error(error: impl std::fmt::Display) -> Error {
    Error::new(
        CaptureErrorCode::WebRtcTrackFailed,
        format!("Windows OpenH264 encoder failed: {error}"),
        true,
    )
}

fn ffmpeg_error(error: impl std::fmt::Display) -> Error {
    Error::new(
        CaptureErrorCode::WebRtcTrackFailed,
        format!("Windows FFmpeg hardware H264 encoder failed: {error}"),
        true,
    )
}
