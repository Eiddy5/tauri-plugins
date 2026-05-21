use std::{
    io::{Read, Write},
    mem::ManuallyDrop,
    ptr,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use openh264::{
    encoder::{Encoder, EncoderConfig, RateControlMode},
    formats::{BgraSliceU8, YUVBuffer},
    Timestamp,
};
use windows::{
    core::{Error as WindowsError, GUID, Interface},
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1},
            Direct3D10::ID3D10Multithread,
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
            },
        },
        Media::MediaFoundation::*,
        System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED},
    },
};

use crate::{
    error::Error,
    models::{CaptureErrorCode, PixelFormat},
    pipeline::frame::VideoFrame,
    webrtc::track::EncodedVideoSample,
    Result,
};

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
    encoder_name: &'static str,
    child: Child,
    stdin: ChildStdin,
    stdout_rx: Receiver<Vec<u8>>,
    width: u32,
    height: u32,
    frame_duration: Duration,
}

impl FfmpegHardwareH264Encoder {
    fn new(width: u32, height: u32, fps: u32) -> Result<Self> {
        validate_bgra_encoder_input(width, height, "FFmpeg hardware")?;
        let ffmpeg = locate_ffmpeg()?;
        let available = ffmpeg_encoders(&ffmpeg)?;
        let candidates = ["h264_amf", "h264_qsv", "h264_nvenc"];
        for encoder_name in candidates {
            if !available.contains(encoder_name) {
                continue;
            }
            match probe_ffmpeg_encoder(&ffmpeg, encoder_name, width, height, fps) {
                Ok(()) => {
                    eprintln!(
                        "[screen-capture] Windows FFmpeg hardware H264 encoder selected: {encoder_name}"
                    );
                    return Self::spawn(ffmpeg, encoder_name, width, height, fps);
                }
                Err(error) => {
                    let message = error.payload().message;
                    eprintln!(
                        "[screen-capture] Windows FFmpeg hardware H264 encoder probe failed for {encoder_name}: {message}"
                    );
                }
            }
        }
        Err(Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            "No usable FFmpeg hardware H264 encoder found. Expected one of h264_amf, h264_qsv, h264_nvenc.",
            true,
        ))
    }

    fn spawn(
        ffmpeg: String,
        encoder_name: &'static str,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self> {
        let mut command = Command::new(ffmpeg);
        command.args(ffmpeg_realtime_args(encoder_name, width, height, fps));
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
        Ok(Self {
            encoder_name,
            child,
            stdin,
            stdout_rx,
            width,
            height,
            frame_duration: Duration::from_secs_f64(1.0 / f64::from(fps.max(1))),
        })
    }

    fn force_keyframe(&mut self) -> Result<()> {
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
        self.stdin.write_all(&frame.data).map_err(ffmpeg_error)?;
        self.stdin.flush().map_err(ffmpeg_error)?;

        let mut data = Vec::new();
        while let Ok(chunk) = self.stdout_rx.try_recv() {
            data.extend_from_slice(&chunk);
        }
        if data.is_empty() {
            return Ok(None);
        }

        Ok(Some(EncodedVideoSample {
            data: h264_to_annex_b(&data),
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
            self.encoder_name
        );
    }
}

struct MediaFoundationH264Encoder {
    transform: IMFTransform,
    width: u32,
    height: u32,
    fps: u32,
    frame_duration: Duration,
    _d3d_manager: Option<IMFDXGIDeviceManager>,
    output_sample_size: u32,
    output_allocates_samples: bool,
}

impl MediaFoundationH264Encoder {
    fn new(width: u32, height: u32, fps: u32) -> Result<Self> {
        validate_bgra_encoder_input(width, height, "Media Foundation")?;
        initialize_media_foundation()?;

        let (transform, d3d_manager, output_sample_size, output_allocates_samples) =
            create_working_h264_transform(width, height, fps)?;

        Ok(Self {
            transform,
            width,
            height,
            fps: fps.max(1),
            frame_duration: Duration::from_secs_f64(1.0 / f64::from(fps.max(1))),
            _d3d_manager: d3d_manager,
            output_sample_size,
            output_allocates_samples,
        })
    }

    fn force_keyframe(&mut self) -> Result<()> {
        *self = Self::new(self.width, self.height, self.fps)?;
        Ok(())
    }

    fn encode_frame(&mut self, frame: &VideoFrame) -> Result<Option<EncodedVideoSample>> {
        if frame.pixel_format != PixelFormat::Bgra {
            return Err(Error::new(
                CaptureErrorCode::WebRtcTrackFailed,
                "Windows Media Foundation encoder expects BGRA frames",
                true,
            ));
        }
        if frame.width != self.width || frame.height != self.height {
            return Err(Error::new(
                CaptureErrorCode::WebRtcTrackFailed,
                format!(
                    "Windows Media Foundation encoder frame size changed from {}x{} to {}x{}",
                    self.width, self.height, frame.width, frame.height
                ),
                true,
            ));
        }

        let nv12 = bgra_to_nv12(frame)?;
        let sample = create_sample_from_bytes(&nv12, frame.timestamp_ns, self.frame_duration)?;

        unsafe {
            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|error| mf_error("ProcessInput failed", error))?;
        }

        let Some(data) = drain_encoder_output(
            &self.transform,
            self.output_sample_size,
            self.output_allocates_samples,
        )?
        else {
            return Ok(None);
        };
        let data = h264_to_annex_b(&data);
        if data.is_empty() {
            Ok(None)
        } else {
            Ok(Some(EncodedVideoSample {
                data,
                duration: self.frame_duration,
                timestamp_ns: frame.timestamp_ns,
            }))
        }
    }
}

struct SoftwareH264Encoder {
    encoder: Encoder,
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

        let source = BgraSliceU8::new(&frame.data, (frame.width as usize, frame.height as usize));
        let yuv = YUVBuffer::from_rgb_source(source);
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

fn initialize_media_foundation() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|error| mf_error("MFStartup failed", error))
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
    encoder_name: &'static str,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<()> {
    let mut child = Command::new(ffmpeg)
        .args(ffmpeg_probe_args(encoder_name, width, height, fps))
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
    encoder_name: &'static str,
    width: u32,
    height: u32,
    fps: u32,
) -> Vec<String> {
    let mut args = ffmpeg_input_args(width, height, fps);
    args.extend(["-frames:v".into(), "1".into()]);
    args.extend(ffmpeg_encoder_args(encoder_name, width, height, fps));
    args.extend(["-f".into(), "h264".into(), "pipe:1".into()]);
    args
}

fn ffmpeg_realtime_args(
    encoder_name: &'static str,
    width: u32,
    height: u32,
    fps: u32,
) -> Vec<String> {
    let mut args = ffmpeg_input_args(width, height, fps);
    args.extend(ffmpeg_encoder_args(encoder_name, width, height, fps));
    args.extend(["-f".into(), "h264".into(), "pipe:1".into()]);
    args
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
    encoder_name: &'static str,
    width: u32,
    height: u32,
    fps: u32,
) -> Vec<String> {
    let bitrate = recommended_bitrate(width, height, fps).to_string();
    let mut args = vec!["-c:v".into(), encoder_name.into()];
    match encoder_name {
        "h264_amf" => args.extend([
            "-usage".into(),
            "lowlatency".into(),
            "-quality".into(),
            "speed".into(),
            "-rc".into(),
            "cbr".into(),
        ]),
        "h264_qsv" => args.extend(["-preset".into(), "veryfast".into()]),
        "h264_nvenc" => args.extend([
            "-preset".into(),
            "p1".into(),
            "-tune".into(),
            "ull".into(),
            "-rc".into(),
            "cbr".into(),
        ]),
        _ => {}
    }
    args.extend([
        "-b:v".into(),
        bitrate.clone(),
        "-maxrate".into(),
        bitrate.clone(),
        "-bufsize".into(),
        (recommended_bitrate(width, height, fps).saturating_mul(2)).to_string(),
        "-g".into(),
        fps.max(1).saturating_mul(2).to_string(),
        "-bf".into(),
        "0".into(),
        "-pix_fmt".into(),
        "nv12".into(),
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

fn create_working_h264_transform(
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(IMFTransform, Option<IMFDXGIDeviceManager>, u32, bool)> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let flags = MFT_ENUM_FLAG(
        MFT_ENUM_FLAG_HARDWARE.0
            | MFT_ENUM_FLAG_SORTANDFILTER.0
            | MFT_ENUM_FLAG_ASYNCMFT.0
            | MFT_ENUM_FLAG_SYNCMFT.0,
    );
    let mut activates: *mut Option<IMFActivate> = ptr::null_mut();
    let mut count = 0;
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            Some(&input),
            Some(&output),
            &mut activates,
            &mut count,
        )
        .map_err(|error| mf_error("MFTEnumEx H264 encoder failed", error))?;
    }

    if activates.is_null() || count == 0 {
        return Err(Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            "No Windows Media Foundation H264 encoder is available",
            true,
        ));
    }

    let mut first_error = None;
    unsafe {
        let raw_activates = std::slice::from_raw_parts_mut(activates, count as usize);
        for activate in raw_activates.iter_mut().filter_map(Option::take) {
            let name = activate_friendly_name(&activate);
            match activate.ActivateObject::<IMFTransform>() {
                Ok(transform) => {
                    match prepare_h264_transform(transform, width, height, fps) {
                        Ok(prepared) => {
                            eprintln!(
                                "[screen-capture] Windows Media Foundation H264 encoder selected: {}",
                                name
                            );
                            CoTaskMemFree(Some(activates.cast()));
                            return Ok(prepared);
                        }
                        Err(error) => {
                            let message = error.payload().message;
                            eprintln!(
                                "[screen-capture] Windows Media Foundation H264 encoder probe failed for {}: {}",
                                name,
                                message
                            );
                            first_error.get_or_insert_with(|| {
                                WindowsError::new(
                                    windows::core::HRESULT(0x8000FFFF_u32 as i32),
                                    message,
                                )
                            });
                        }
                    }
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        CoTaskMemFree(Some(activates.cast()));
    }

    if let Some(error) = first_error {
        Err(mf_error(
            "Activating Windows Media Foundation H264 encoder failed",
            error,
        ))
    } else {
        Err(Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            "Activating Windows Media Foundation H264 encoder failed",
            true,
        ))
    }
}

fn prepare_h264_transform(
    transform: IMFTransform,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(IMFTransform, Option<IMFDXGIDeviceManager>, u32, bool)> {
    unlock_async_transform_if_needed(&transform);
    let d3d_manager = attach_d3d_manager(&transform);
    configure_media_foundation_encoder(&transform, width, height, fps)?;

    let output_info = unsafe { transform.GetOutputStreamInfo(0) }
        .map_err(|error| mf_error("GetOutputStreamInfo failed", error))?;
    let output_sample_size = output_info
        .cbSize
        .max(recommended_bitrate(width, height, fps).saturating_div(8).max(1));
    let output_allocates_samples = has_flag(
        output_info.dwFlags,
        MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32,
    ) || has_flag(
        output_info.dwFlags,
        MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32,
    );

    unsafe {
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .map_err(|error| mf_error("begin streaming failed", error))?;
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .map_err(|error| mf_error("start of stream failed", error))?;
    }

    probe_h264_transform(
        &transform,
        width,
        height,
        Duration::from_secs_f64(1.0 / f64::from(fps.max(1))),
        output_sample_size,
        output_allocates_samples,
    )?;

    Ok((transform, d3d_manager, output_sample_size, output_allocates_samples))
}

fn attach_d3d_manager(transform: &IMFTransform) -> Option<IMFDXGIDeviceManager> {
    match create_d3d_manager() {
        Ok(manager) => {
            let result = unsafe {
                transform.ProcessMessage(
                    MFT_MESSAGE_SET_D3D_MANAGER,
                    manager.as_raw() as usize,
                )
            };
            match result {
                Ok(()) => {
                    eprintln!("[screen-capture] Windows Media Foundation H264 encoder attached D3D11 device manager");
                    Some(manager)
                }
                Err(error) => {
                    eprintln!(
                        "[screen-capture] Windows Media Foundation H264 encoder ignored D3D11 device manager: {error}"
                    );
                    None
                }
            }
        }
        Err(error) => {
            let message = error.payload().message;
            eprintln!(
                "[screen-capture] Windows Media Foundation H264 encoder D3D11 device manager unavailable: {message}"
            );
            None
        }
    }
}

fn create_d3d_manager() -> Result<IMFDXGIDeviceManager> {
    let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
    let mut device: Option<ID3D11Device> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            flags,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
        .map_err(|error| mf_error("D3D11CreateDevice failed", error))?;
    }
    let device = device.ok_or_else(|| {
        Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            "D3D11CreateDevice returned no device",
            true,
        )
    })?;
    if let Ok(multithread) = device.cast::<ID3D10Multithread>() {
        unsafe {
            let _ = multithread.SetMultithreadProtected(true);
        }
    }

    let mut reset_token = 0;
    let mut manager = None;
    unsafe {
        MFCreateDXGIDeviceManager(&mut reset_token, &mut manager)
            .map_err(|error| mf_error("MFCreateDXGIDeviceManager failed", error))?;
    }
    let manager = manager.ok_or_else(|| {
        Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            "MFCreateDXGIDeviceManager returned no manager",
            true,
        )
    })?;
    unsafe {
        manager
            .ResetDevice(&device.cast::<windows::core::IUnknown>().map_err(|error| {
                mf_error("cast D3D11 device to IUnknown failed", error)
            })?, reset_token)
            .map_err(|error| mf_error("DXGI device manager ResetDevice failed", error))?;
    }
    Ok(manager)
}

fn probe_h264_transform(
    transform: &IMFTransform,
    width: u32,
    height: u32,
    frame_duration: Duration,
    output_sample_size: u32,
    output_allocates_samples: bool,
) -> Result<()> {
    let nv12_len = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(3)
        / 2;
    let black = vec![0u8; nv12_len];
    for index in 0..3 {
        let sample = create_sample_from_bytes(
            &black,
            (index as u64).saturating_mul(frame_duration.as_nanos() as u64),
            frame_duration,
        )?;
        unsafe {
            transform
                .ProcessInput(0, &sample, 0)
                .map_err(|error| mf_error("probe ProcessInput failed", error))?;
        }
        let _ = drain_encoder_output(transform, output_sample_size, output_allocates_samples)?;
    }
    Ok(())
}

fn activate_friendly_name(activate: &IMFActivate) -> String {
    unsafe {
        let mut value = windows::core::PWSTR::null();
        let mut len = 0;
        if activate
            .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut value, &mut len)
            .is_ok()
            && !value.is_null()
        {
            let name = value.to_string().unwrap_or_else(|_| "unknown".to_string());
            CoTaskMemFree(Some(value.as_ptr().cast()));
            return name;
        }
    }
    "unknown".to_string()
}

fn unlock_async_transform_if_needed(transform: &IMFTransform) {
    unsafe {
        if let Ok(attributes) = transform.GetAttributes() {
            let _ = attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1);
        }
    }
}

fn configure_media_foundation_encoder(
    transform: &IMFTransform,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<()> {
    let output_type = create_video_type(
        MFVideoFormat_H264,
        width,
        height,
        fps,
        Some(recommended_bitrate(width, height, fps)),
    )?;
    unsafe {
        let _ = output_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32);
        let _ = output_type.SetUINT32(&MF_LOW_LATENCY, 1);
        let _ = transform.SetOutputType(0, &output_type, 0);
    }

    let input_type = create_video_type(MFVideoFormat_NV12, width, height, fps, None)?;
    unsafe {
        transform
            .SetInputType(0, &input_type, 0)
            .map_err(|error| mf_error("SetInputType NV12 failed", error))?;
        transform
            .SetOutputType(0, &output_type, 0)
            .map_err(|error| mf_error("SetOutputType H264 failed", error))?;
    }
    Ok(())
}

fn create_video_type(
    subtype: GUID,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: Option<u32>,
) -> Result<IMFMediaType> {
    let media_type =
        unsafe { MFCreateMediaType() }.map_err(|error| mf_error("MFCreateMediaType failed", error))?;
    unsafe {
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|error| mf_error("Set media major type failed", error))?;
        media_type
            .SetGUID(&MF_MT_SUBTYPE, &subtype)
            .map_err(|error| mf_error("Set media subtype failed", error))?;
        media_type
            .SetUINT64(&MF_MT_FRAME_SIZE, ratio_u64(width, height))
            .map_err(|error| mf_error("Set frame size failed", error))?;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, ratio_u64(fps.max(1), 1))
            .map_err(|error| mf_error("Set frame rate failed", error))?;
        media_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, ratio_u64(1, 1))
            .map_err(|error| mf_error("Set pixel aspect ratio failed", error))?;
        media_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|error| mf_error("Set interlace mode failed", error))?;
        if let Some(bitrate) = bitrate {
            media_type
                .SetUINT32(&MF_MT_AVG_BITRATE, bitrate)
                .map_err(|error| mf_error("Set average bitrate failed", error))?;
        }
    }
    Ok(media_type)
}

fn create_sample_from_bytes(
    bytes: &[u8],
    timestamp_ns: u64,
    duration: Duration,
) -> Result<IMFSample> {
    let buffer = unsafe { MFCreateMemoryBuffer(bytes.len() as u32) }
        .map_err(|error| mf_error("MFCreateMemoryBuffer failed", error))?;
    unsafe {
        let mut data = ptr::null_mut();
        buffer
            .Lock(&mut data, None, None)
            .map_err(|error| mf_error("lock Media Foundation buffer failed", error))?;
        ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
        buffer
            .Unlock()
            .map_err(|error| mf_error("unlock Media Foundation buffer failed", error))?;
        buffer
            .SetCurrentLength(bytes.len() as u32)
            .map_err(|error| mf_error("set Media Foundation buffer length failed", error))?;

        let sample = MFCreateSample().map_err(|error| mf_error("MFCreateSample failed", error))?;
        sample
            .AddBuffer(&buffer)
            .map_err(|error| mf_error("add Media Foundation buffer failed", error))?;
        sample
            .SetSampleTime((timestamp_ns / 100) as i64)
            .map_err(|error| mf_error("set sample time failed", error))?;
        sample
            .SetSampleDuration(duration.as_nanos().saturating_div(100) as i64)
            .map_err(|error| mf_error("set sample duration failed", error))?;
        Ok(sample)
    }
}

fn drain_encoder_output(
    transform: &IMFTransform,
    output_sample_size: u32,
    output_allocates_samples: bool,
) -> Result<Option<Vec<u8>>> {
    let sample = if output_allocates_samples {
        None
    } else {
        let sample = unsafe { MFCreateSample() }
            .map_err(|error| mf_error("MFCreateSample output failed", error))?;
        let buffer = unsafe { MFCreateMemoryBuffer(output_sample_size) }
            .map_err(|error| mf_error("MFCreateMemoryBuffer output failed", error))?;
        unsafe {
            sample
                .AddBuffer(&buffer)
                .map_err(|error| mf_error("add output buffer failed", error))?;
        }
        Some(sample)
    };

    let mut output = MFT_OUTPUT_DATA_BUFFER {
        dwStreamID: 0,
        pSample: ManuallyDrop::new(sample),
        dwStatus: 0,
        pEvents: ManuallyDrop::new(None),
    };
    let mut status = 0;
    let result = unsafe { transform.ProcessOutput(0, std::slice::from_mut(&mut output), &mut status) };

    match result {
        Ok(()) => {
            let sample = unsafe { ManuallyDrop::take(&mut output.pSample) };
            let _events = unsafe { ManuallyDrop::take(&mut output.pEvents) };
            if has_flag(output.dwStatus, MFT_OUTPUT_DATA_BUFFER_NO_SAMPLE.0 as u32) {
                Ok(None)
            } else if let Some(sample) = sample {
                sample_to_bytes(&sample).map(Some)
            } else {
                Ok(None)
            }
        }
        Err(error)
            if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT
                || error.code() == MF_E_NOTACCEPTING
                || error.code() == MF_E_TRANSFORM_STREAM_CHANGE =>
        {
            unsafe {
                let _ = ManuallyDrop::take(&mut output.pSample);
                let _ = ManuallyDrop::take(&mut output.pEvents);
            }
            Ok(None)
        }
        Err(error) => {
            unsafe {
                let _ = ManuallyDrop::take(&mut output.pSample);
                let _ = ManuallyDrop::take(&mut output.pEvents);
            }
            Err(mf_error("ProcessOutput failed", error))
        }
    }
}

fn sample_to_bytes(sample: &IMFSample) -> Result<Vec<u8>> {
    let buffer = unsafe { sample.ConvertToContiguousBuffer() }
        .map_err(|error| mf_error("ConvertToContiguousBuffer failed", error))?;
    let mut data = ptr::null_mut();
    let mut current_length = 0;
    unsafe {
        buffer
            .Lock(&mut data, None, Some(&mut current_length))
            .map_err(|error| mf_error("lock encoded buffer failed", error))?;
        let bytes = std::slice::from_raw_parts(data, current_length as usize).to_vec();
        buffer
            .Unlock()
            .map_err(|error| mf_error("unlock encoded buffer failed", error))?;
        Ok(bytes)
    }
}

fn bgra_to_nv12(frame: &VideoFrame) -> Result<Vec<u8>> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| {
            Error::new(
                CaptureErrorCode::WebRtcTrackFailed,
                "Windows frame size overflowed while converting BGRA to NV12",
                true,
            )
        })?;
    if frame.data.len() < expected {
        return Err(Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            format!(
                "Windows frame has {} bytes, expected at least {}",
                frame.data.len(),
                expected
            ),
            true,
        ));
    }

    let y_size = width * height;
    let mut nv12 = vec![0u8; y_size + y_size / 2];
    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 4;
            let b = frame.data[src] as i32;
            let g = frame.data[src + 1] as i32;
            let r = frame.data[src + 2] as i32;
            nv12[y * width + x] = y_from_rgb(r, g, b);
        }
    }

    let uv_offset = y_size;
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let mut u_sum = 0;
            let mut v_sum = 0;
            let mut count = 0;
            for yy in y..(y + 2).min(height) {
                for xx in x..(x + 2).min(width) {
                    let src = (yy * width + xx) * 4;
                    let b = frame.data[src] as i32;
                    let g = frame.data[src + 1] as i32;
                    let r = frame.data[src + 2] as i32;
                    let (u, v) = uv_from_rgb(r, g, b);
                    u_sum += u as i32;
                    v_sum += v as i32;
                    count += 1;
                }
            }
            let dst = uv_offset + (y / 2) * width + x;
            nv12[dst] = (u_sum / count).clamp(0, 255) as u8;
            nv12[dst + 1] = (v_sum / count).clamp(0, 255) as u8;
        }
    }
    Ok(nv12)
}

fn h264_to_annex_b(input: &[u8]) -> Vec<u8> {
    if input.starts_with(&[0, 0, 0, 1]) || input.starts_with(&[0, 0, 1]) {
        let mut output = Vec::with_capacity(input.len());
        let mut offset = 0;
        while offset < input.len() {
            if input[offset..].starts_with(&[0, 0, 0, 1]) {
                offset += 4;
            } else if input[offset..].starts_with(&[0, 0, 1]) {
                offset += 3;
            } else {
                offset += 1;
                continue;
            }
            let next = find_next_start_code(input, offset).unwrap_or(input.len());
            append_annex_b_nal(&mut output, &input[offset..next]);
            offset = next;
        }
        return output;
    }

    let mut output = Vec::with_capacity(input.len() + 16);
    let mut offset = 0;
    while offset + 4 <= input.len() {
        let len = u32::from_be_bytes(input[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        if len == 0 || offset + len > input.len() {
            return input.to_vec();
        }
        append_annex_b_nal(&mut output, &input[offset..offset + len]);
        offset += len;
    }
    if output.is_empty() {
        input.to_vec()
    } else {
        output
    }
}

fn find_next_start_code(input: &[u8], start: usize) -> Option<usize> {
    let mut offset = start;
    while offset + 3 <= input.len() {
        if input[offset..].starts_with(&[0, 0, 0, 1]) || input[offset..].starts_with(&[0, 0, 1]) {
            return Some(offset);
        }
        offset += 1;
    }
    None
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

fn ratio_u64(numerator: u32, denominator: u32) -> u64 {
    (u64::from(numerator) << 32) | u64::from(denominator)
}

fn has_flag(flags: u32, flag: u32) -> bool {
    flags & flag != 0
}

fn y_from_rgb(r: i32, g: i32, b: i32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8
}

fn uv_from_rgb(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
    let v = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
    (u, v)
}

fn recommended_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    let fps = u64::from(fps.max(1));
    let bits = pixels
        .saturating_mul(fps)
        .saturating_mul(32)
        .saturating_div(100);
    u32::try_from(bits.clamp(8_000_000, 48_000_000)).unwrap_or(24_000_000)
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

fn mf_error(message: impl AsRef<str>, error: WindowsError) -> Error {
    Error::new(
        CaptureErrorCode::WebRtcTrackFailed,
        format!(
            "Windows Media Foundation H264 encoder failed: {}: {error}",
            message.as_ref()
        ),
        true,
    )
}
