use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{
    error::Error,
    models::{
        AgoraPublisherOptions, CaptureErrorCode, CaptureStats, PixelFormat, StartCaptureOptions,
    },
    pipeline::frame::VideoFrame,
    publisher::CapturePublisher,
    Result,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgoraExternalVideoPixelFormat {
    Bgra,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgoraExternalVideoFrame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixel_format: AgoraExternalVideoPixelFormat,
    pub timestamp_us: u64,
    pub data: Arc<[u8]>,
}

impl TryFrom<VideoFrame> for AgoraExternalVideoFrame {
    type Error = Error;

    fn try_from(frame: VideoFrame) -> Result<Self> {
        if frame.pixel_format != PixelFormat::Bgra {
            return Err(Error::new(
                CaptureErrorCode::PublisherUnsupported,
                "Agora publisher currently expects tightly packed BGRA frames",
                true,
            ));
        }

        let stride = frame.width.checked_mul(4).ok_or_else(|| {
            Error::new(
                CaptureErrorCode::CaptureRuntimeFailed,
                "BGRA frame stride overflowed u32",
                true,
            )
        })?;
        let expected_len = usize::try_from(stride)
            .ok()
            .and_then(|stride| {
                usize::try_from(frame.height)
                    .ok()
                    .and_then(|height| stride.checked_mul(height))
            })
            .ok_or_else(|| {
                Error::new(
                    CaptureErrorCode::CaptureRuntimeFailed,
                    "BGRA frame byte length overflowed usize",
                    true,
                )
            })?;
        if frame.data.len() != expected_len {
            return Err(Error::new(
                CaptureErrorCode::CaptureRuntimeFailed,
                format!(
                    "BGRA frame data length {} does not match expected {}",
                    frame.data.len(),
                    expected_len
                ),
                true,
            ));
        }

        Ok(Self {
            width: frame.width,
            height: frame.height,
            stride,
            pixel_format: AgoraExternalVideoPixelFormat::Bgra,
            timestamp_us: frame.timestamp_ns / 1_000,
            data: frame.data,
        })
    }
}

#[async_trait]
pub trait AgoraVideoSink: Send + Sync {
    async fn start(
        &self,
        agora: &AgoraPublisherOptions,
        capture: &StartCaptureOptions,
    ) -> Result<()>;
    async fn push_external_video_frame(&self, frame: AgoraExternalVideoFrame) -> Result<()>;
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn stats(&self) -> Result<CaptureStats>;
}

pub struct AgoraPublisher {
    options: AgoraPublisherOptions,
    sink: Arc<dyn AgoraVideoSink>,
    stats: Mutex<CaptureStats>,
}

impl AgoraPublisher {
    pub fn new(options: AgoraPublisherOptions) -> Self {
        Self::with_sink(options, Arc::new(UnsupportedAgoraVideoSink))
    }

    pub fn with_sink(options: AgoraPublisherOptions, sink: Arc<dyn AgoraVideoSink>) -> Self {
        Self {
            options,
            sink,
            stats: Mutex::new(CaptureStats::default()),
        }
    }

    pub fn sdk_unavailable_error(options: &AgoraPublisherOptions) -> Error {
        Error::new(
            CaptureErrorCode::PublisherUnsupported,
            format!(
                "Agora publisher is configured for channel '{}' but the native Agora SDK adapter is not linked yet",
                options.channel
            ),
            true,
        )
    }
}

#[async_trait]
impl CapturePublisher for AgoraPublisher {
    async fn start(&self, options: StartCaptureOptions) -> Result<()> {
        self.sink.start(&self.options, &options).await?;
        let mut stats = self.stats.lock().await;
        stats.started = true;
        Ok(())
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        let external = match AgoraExternalVideoFrame::try_from(frame) {
            Ok(frame) => frame,
            Err(error) => {
                self.stats.lock().await.frames_dropped += 1;
                return Err(error);
            }
        };
        self.sink.push_external_video_frame(external).await?;
        let mut stats = self.stats.lock().await;
        stats.frames_published += 1;
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        self.sink.pause().await?;
        self.stats.lock().await.started = false;
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        self.sink.resume().await?;
        self.stats.lock().await.started = true;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.sink.stop().await?;
        self.stats.lock().await.started = false;
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        let mut stats = self.stats.lock().await.clone();
        let sink_stats = self.sink.stats().await?;
        stats.frames_published = stats.frames_published.max(sink_stats.frames_published);
        stats.frames_dropped = stats.frames_dropped.max(sink_stats.frames_dropped);
        stats.bitrate_kbps = stats.bitrate_kbps.max(sink_stats.bitrate_kbps);
        stats.fps = stats.fps.max(sink_stats.fps);
        stats.started |= sink_stats.started;
        Ok(stats)
    }
}

#[derive(Debug)]
struct UnsupportedAgoraVideoSink;

#[async_trait]
impl AgoraVideoSink for UnsupportedAgoraVideoSink {
    async fn start(
        &self,
        agora: &AgoraPublisherOptions,
        _capture: &StartCaptureOptions,
    ) -> Result<()> {
        Err(AgoraPublisher::sdk_unavailable_error(agora))
    }

    async fn push_external_video_frame(&self, _frame: AgoraExternalVideoFrame) -> Result<()> {
        Err(Error::new(
            CaptureErrorCode::PublisherUnsupported,
            "Agora native SDK adapter is not linked yet",
            true,
        ))
    }

    async fn pause(&self) -> Result<()> {
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        Err(Error::new(
            CaptureErrorCode::PublisherUnsupported,
            "Agora native SDK adapter is not linked yet",
            true,
        ))
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(CaptureStats::default())
    }
}
