use bytes::Bytes;
use std::{sync::Arc, time::Duration};

use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use crate::{error::Error, models::CaptureErrorCode, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedVideoSample {
    pub data: Vec<u8>,
    pub duration: Duration,
    pub timestamp_ns: u64,
}

#[derive(Clone)]
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
