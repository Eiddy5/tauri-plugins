use bytes::{BufMut, Bytes, BytesMut};
use std::{sync::Arc, time::Duration};

use tokio::sync::Mutex;
use webrtc::data_channel::{data_channel_state::RTCDataChannelState, RTCDataChannel};
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use crate::{
    error::Error,
    models::{CaptureErrorCode, PixelFormat},
    pipeline::frame::VideoFrame,
    Result,
};

const FRAME_MAGIC: &[u8; 4] = b"SCF1";
const FRAME_CHUNK_MAGIC: &[u8; 4] = b"SCF2";
const HEADER_LEN: usize = 25;
const CHUNK_HEADER_LEN: usize = 41;
pub const MAX_DATA_CHANNEL_MESSAGE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedVideoSample {
    pub data: Vec<u8>,
    pub duration: Duration,
    pub timestamp_ns: u64,
}

#[derive(Debug, Clone)]
pub struct WebRtcFramePacket {
    frame: VideoFrame,
}

impl WebRtcFramePacket {
    pub fn from_frame(frame: VideoFrame) -> Result<Self> {
        if frame.data.is_empty() {
            return Err(Error::new(
                CaptureErrorCode::WebRtcTrackFailed,
                "captured frame has no pixel data",
                true,
            ));
        }
        Ok(Self { frame })
    }

    pub fn into_bytes(self) -> Bytes {
        let mut bytes = BytesMut::with_capacity(HEADER_LEN + self.frame.data.len());
        bytes.put_slice(FRAME_MAGIC);
        bytes.put_u32_le(self.frame.width);
        bytes.put_u32_le(self.frame.height);
        bytes.put_u8(pixel_format_code(self.frame.pixel_format));
        bytes.put_u64_le(self.frame.timestamp_ns);
        bytes.put_u32_le(u32::try_from(self.frame.data.len()).unwrap_or(u32::MAX));
        bytes.put_slice(&self.frame.data);
        bytes.freeze()
    }

    pub fn into_chunks(self) -> Vec<Bytes> {
        let chunk_payload_len = MAX_DATA_CHANNEL_MESSAGE_BYTES.saturating_sub(CHUNK_HEADER_LEN);
        let chunk_count = self.frame.data.len().div_ceil(chunk_payload_len);
        let chunk_count_u32 = u32::try_from(chunk_count).unwrap_or(u32::MAX);
        let total_len = u32::try_from(self.frame.data.len()).unwrap_or(u32::MAX);

        self.frame
            .data
            .chunks(chunk_payload_len)
            .enumerate()
            .map(|(index, chunk)| {
                let offset = index * chunk_payload_len;
                let mut bytes = BytesMut::with_capacity(CHUNK_HEADER_LEN + chunk.len());
                bytes.put_slice(FRAME_CHUNK_MAGIC);
                bytes.put_u32_le(self.frame.width);
                bytes.put_u32_le(self.frame.height);
                bytes.put_u8(pixel_format_code(self.frame.pixel_format));
                bytes.put_u64_le(self.frame.timestamp_ns);
                bytes.put_u32_le(total_len);
                bytes.put_u32_le(u32::try_from(index).unwrap_or(u32::MAX));
                bytes.put_u32_le(chunk_count_u32);
                bytes.put_u32_le(u32::try_from(offset).unwrap_or(u32::MAX));
                bytes.put_u32_le(u32::try_from(chunk.len()).unwrap_or(u32::MAX));
                bytes.put_slice(chunk);
                bytes.freeze()
            })
            .collect()
    }
}

pub struct WebRtcFrameSender {
    data_channel: Arc<RTCDataChannel>,
    pending_frame: Arc<Mutex<Option<VideoFrame>>>,
}

pub struct WebRtcH264SampleSender {
    video_track: Arc<TrackLocalStaticSample>,
}

impl WebRtcH264SampleSender {
    pub fn new(video_track: Arc<TrackLocalStaticSample>) -> Self {
        Self { video_track }
    }

    pub async fn send_sample(&self, sample: EncodedVideoSample) -> Result<()> {
        if sample.data.is_empty() {
            return Err(Error::new(
                CaptureErrorCode::WebRtcTrackFailed,
                "cannot publish empty encoded video sample",
                true,
            ));
        }

        self.video_track
            .write_sample(&media::Sample {
                data: Bytes::from(sample.data),
                duration: sample.duration,
                ..media::Sample::default()
            })
            .await
            .map_err(|err| {
                Error::new(
                    CaptureErrorCode::WebRtcTrackFailed,
                    format!("failed to write H264 sample to WebRTC video track: {err}"),
                    true,
                )
            })
    }
}

impl WebRtcFrameSender {
    pub fn new(data_channel: Arc<RTCDataChannel>) -> Self {
        let pending_frame = Arc::new(Mutex::new(None));
        data_channel.on_open(Box::new({
            let data_channel = Arc::clone(&data_channel);
            let pending_frame = Arc::clone(&pending_frame);
            move || {
                Box::pin(async move {
                    let frame = pending_frame.lock().await.take();
                    if let Some(frame) = frame {
                        let _ = send_frame_chunks(&data_channel, frame).await;
                    }
                })
            }
        }));

        Self {
            data_channel,
            pending_frame,
        }
    }

    pub async fn send_frame(&self, frame: VideoFrame) -> Result<bool> {
        if self.data_channel.ready_state() != RTCDataChannelState::Open {
            *self.pending_frame.lock().await = Some(frame);
            return Ok(false);
        }

        if self.data_channel.buffered_amount().await > MAX_DATA_CHANNEL_MESSAGE_BYTES {
            *self.pending_frame.lock().await = Some(frame);
            return Ok(false);
        }

        send_frame_chunks(&self.data_channel, frame).await?;
        Ok(true)
    }
}

async fn send_frame_chunks(data_channel: &RTCDataChannel, frame: VideoFrame) -> Result<()> {
    for chunk in WebRtcFramePacket::from_frame(frame)?.into_chunks() {
        data_channel.send(&chunk).await.map_err(|err| {
            Error::new(
                CaptureErrorCode::WebRtcTrackFailed,
                format!("failed to send WebRTC frame packet: {err}"),
                true,
            )
        })?;
    }
    Ok(())
}

fn pixel_format_code(pixel_format: PixelFormat) -> u8 {
    match pixel_format {
        PixelFormat::Bgra => 0,
        PixelFormat::Rgba => 1,
        PixelFormat::Nv12 => 2,
    }
}
