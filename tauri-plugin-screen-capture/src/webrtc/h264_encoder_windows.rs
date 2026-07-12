use std::time::Duration;

use openh264::{
    encoder::{Encoder, EncoderConfig, RateControlMode},
    formats::YUVBuffer,
    Timestamp,
};
use yuvutils_rs::{bgra_to_yuv420, YuvRange, YuvStandardMatrix};

use crate::{
    error::Error,
    models::{CaptureErrorCode, PixelFormat},
    pipeline::frame::VideoFrame,
    platform::windows::media::{
        media_foundation::MediaFoundationH264Encoder, recommended_screen_share_bitrate,
        WindowsGpuSurface,
    },
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
    MediaFoundation {
        encoder: MediaFoundationH264Encoder,
        gpu_surface: bool,
    },
    OpenH264(Box<SoftwareH264Encoder>),
}

impl H264Encoder {
    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    pub(crate) fn height(&self) -> u32 {
        self.height
    }

    pub fn new(width: u32, height: u32, fps: u32) -> Result<Self> {
        match MediaFoundationH264Encoder::new(width, height, fps) {
            Ok(encoder) => {
                eprintln!(
                    "[screen-capture] Windows H264 encoder: native Media Foundation hardware MFT"
                );
                Ok(Self {
                    backend: EncoderBackend::MediaFoundation {
                        encoder,
                        gpu_surface: false,
                    },
                    width,
                    height,
                    fps: fps.max(1),
                })
            }
            Err(error) => {
                eprintln!(
                    "[screen-capture] Windows Media Foundation hardware H264 unavailable, falling back to OpenH264: {}",
                    error.payload().message
                );
                Ok(Self {
                    backend: EncoderBackend::OpenH264(Box::new(SoftwareH264Encoder::new(
                        width, height, fps,
                    )?)),
                    width,
                    height,
                    fps: fps.max(1),
                })
            }
        }
    }

    pub fn new_gpu_surface(surface: &WindowsGpuSurface, fps: u32) -> Result<Self> {
        let width = surface.width();
        let height = surface.height();
        let encoder = MediaFoundationH264Encoder::new_gpu_surface(
            width,
            height,
            fps,
            surface.device(),
        )?;
        eprintln!(
            "[screen-capture] Windows H264 encoder: Media Foundation D3D11 surface input"
        );
        Ok(Self {
            backend: EncoderBackend::MediaFoundation {
                encoder,
                gpu_surface: true,
            },
            width,
            height,
            fps: fps.max(1),
        })
    }

    pub fn force_keyframe(&mut self) -> Result<()> {
        match &mut self.backend {
            EncoderBackend::MediaFoundation { encoder, .. } => encoder.force_keyframe(),
            EncoderBackend::OpenH264(encoder) => encoder.force_keyframe(),
        }
    }

    pub(crate) fn set_bitrate_bps(&mut self, bitrate_bps: u32) -> Result<()> {
        match &mut self.backend {
            EncoderBackend::MediaFoundation { encoder, .. } => {
                encoder.set_bitrate_bps(bitrate_bps)
            }
            EncoderBackend::OpenH264(_) => Err(encoder_error(
                "dynamic bitrate updates are unavailable for the OpenH264 fallback",
            )),
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            EncoderBackend::MediaFoundation {
                gpu_surface: true, ..
            } => "media-foundation-d3d11-surface",
            EncoderBackend::MediaFoundation {
                gpu_surface: false,
                ..
            } => "media-foundation-hardware",
            EncoderBackend::OpenH264(_) => "openh264",
        }
    }

    pub fn encode_frame(&mut self, frame: &VideoFrame) -> Result<Option<EncodedVideoSample>> {
        let result = match &mut self.backend {
            EncoderBackend::MediaFoundation { encoder, .. } => encoder.encode_frame(frame),
            EncoderBackend::OpenH264(encoder) => encoder.encode_frame(frame),
        };
        result.or_else(|error| {
            if matches!(self.backend, EncoderBackend::OpenH264(_)) {
                return Err(error);
            }
            eprintln!(
                "[screen-capture] Windows native hardware H264 encoder failed while publishing, falling back to OpenH264: {}",
                error.payload().message
            );
            self.backend = EncoderBackend::OpenH264(Box::new(SoftwareH264Encoder::new(
                self.width,
                self.height,
                self.fps,
            )?));
            match &mut self.backend {
                EncoderBackend::OpenH264(encoder) => encoder.encode_frame(frame),
                EncoderBackend::MediaFoundation { .. } => unreachable!(),
            }
        })
    }

    pub fn encode_gpu_surface(
        &mut self,
        surface: &WindowsGpuSurface,
    ) -> Result<Option<EncodedVideoSample>> {
        match &mut self.backend {
            EncoderBackend::MediaFoundation {
                encoder,
                gpu_surface: true,
            } => encoder.encode_gpu_surface(surface),
            EncoderBackend::MediaFoundation {
                gpu_surface: false,
                ..
            } => Err(encoder_error(
                "Media Foundation encoder is not configured for D3D11 surface input",
            )),
            EncoderBackend::OpenH264(_) => Err(encoder_error(
                "OpenH264 cannot encode a D3D11 surface without CPU readback",
            )),
        }
    }

    pub(crate) fn encode_nv12_readback(
        &mut self,
        nv12: &[u8],
        timestamp_ns: u64,
    ) -> Result<Option<EncodedVideoSample>> {
        let result = match &mut self.backend {
            EncoderBackend::MediaFoundation { encoder, .. } => {
                encoder.encode_nv12_readback(nv12, timestamp_ns)
            }
            EncoderBackend::OpenH264(encoder) => {
                encoder.encode_nv12(nv12, timestamp_ns)
            }
        };
        result.or_else(|error| {
            if matches!(self.backend, EncoderBackend::OpenH264(_)) {
                return Err(error);
            }
            eprintln!(
                "[screen-capture] Windows CPU Media Foundation NV12 encode failed, falling back to OpenH264: {}",
                error.payload().message
            );
            self.backend = EncoderBackend::OpenH264(Box::new(SoftwareH264Encoder::new(
                self.width,
                self.height,
                self.fps,
            )?));
            match &mut self.backend {
                EncoderBackend::OpenH264(encoder) => encoder.encode_nv12(nv12, timestamp_ns),
                EncoderBackend::MediaFoundation { .. } => unreachable!(),
            }
        })
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
        validate_bgra_encoder_input(width, height)?;
        let config = EncoderConfig::new()
            .set_bitrate_bps(recommended_screen_share_bitrate(width, height, fps))
            .max_frame_rate(fps.max(1) as f32)
            .enable_skip_frame(true)
            .rate_control_mode(RateControlMode::Bitrate);
        let encoder = Encoder::with_api_config(openh264::OpenH264API::from_source(), config)
            .map_err(encoder_error)?;
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
            return Err(encoder_error("OpenH264 expects BGRA frames"));
        }
        if frame.width != self.width || frame.height != self.height {
            return Err(encoder_error(format!(
                "OpenH264 frame size changed from {}x{} to {}x{}",
                self.width, self.height, frame.width, frame.height
            )));
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

    fn encode_nv12(
        &mut self,
        nv12: &[u8],
        timestamp_ns: u64,
    ) -> Result<Option<EncodedVideoSample>> {
        let width = self.width as usize;
        let height = self.height as usize;
        let y_len = width * height;
        let uv_len = y_len / 4;
        if nv12.len() != y_len + uv_len * 2 {
            return Err(encoder_error(format!(
                "OpenH264 expected {} NV12 bytes, got {}",
                y_len + uv_len * 2,
                nv12.len()
            )));
        }
        let mut i420 = vec![0u8; y_len + uv_len * 2];
        i420[..y_len].copy_from_slice(&nv12[..y_len]);
        for index in 0..uv_len {
            i420[y_len + index] = nv12[y_len + index * 2];
            i420[y_len + uv_len + index] = nv12[y_len + index * 2 + 1];
        }
        let yuv = YUVBuffer::from_vec(i420, width, height);
        let bitstream = self
            .encoder
            .encode_at(&yuv, Timestamp::from_millis(timestamp_ns / 1_000_000))
            .map_err(encoder_error)?;
        let data = annex_b_from_bitstream(&bitstream);
        if data.is_empty() {
            return Ok(None);
        }
        Ok(Some(EncodedVideoSample {
            data,
            duration: self.frame_duration,
            timestamp_ns,
        }))
    }
}

fn annex_b_from_bitstream(bitstream: &openh264::encoder::EncodedBitStream<'_>) -> Vec<u8> {
    let mut data = Vec::new();
    for layer_index in 0..bitstream.num_layers() {
        let Some(layer) = bitstream.layer(layer_index) else {
            continue;
        };
        for nal_index in 0..layer.nal_count() {
            if let Some(nal) = layer.nal_unit(nal_index) {
                append_annex_b_nal(&mut data, nal);
            }
        }
    }
    data
}

fn append_annex_b_nal(output: &mut Vec<u8>, nal: &[u8]) {
    let payload = nal
        .strip_prefix(&[0, 0, 0, 1])
        .or_else(|| nal.strip_prefix(&[0, 0, 1]))
        .unwrap_or(nal);
    if !payload.is_empty() {
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(payload);
    }
}

fn validate_bgra_encoder_input(width: u32, height: u32) -> Result<()> {
    if width % 2 != 0 || height % 2 != 0 {
        return Err(encoder_error(format!(
            "OpenH264 requires even dimensions, got {width}x{height}"
        )));
    }
    Ok(())
}

fn encoder_error(error: impl std::fmt::Display) -> Error {
    Error::new(
        CaptureErrorCode::WebRtcTrackFailed,
        format!("Windows OpenH264 encoder failed: {error}"),
        true,
    )
}
