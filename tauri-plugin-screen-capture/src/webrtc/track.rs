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

impl EncodedVideoSample {
    #[allow(dead_code)]
    pub(crate) fn is_h264_idr(&self) -> bool {
        let mut offset = 0;
        while offset + 3 < self.data.len() {
            let start_code_len = if self.data[offset..].starts_with(&[0, 0, 0, 1]) {
                4
            } else if self.data[offset..].starts_with(&[0, 0, 1]) {
                3
            } else {
                offset += 1;
                continue;
            };
            let nal_offset = offset + start_code_len;
            if nal_offset < self.data.len() && self.data[nal_offset] & 0x1f == 5 {
                return true;
            }
            offset = nal_offset.saturating_add(1);
        }
        false
    }
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

#[cfg(test)]
mod tests {
    use super::EncodedVideoSample;
    use std::time::Duration;

    fn sample(data: &[u8]) -> EncodedVideoSample {
        EncodedVideoSample {
            data: data.to_vec(),
            duration: Duration::from_millis(16),
            timestamp_ns: 0,
        }
    }

    #[test]
    fn detects_idr_in_annex_b_access_unit() {
        assert!(sample(&[0, 0, 0, 1, 0x67, 1, 0, 0, 1, 0x65, 2]).is_h264_idr());
        assert!(!sample(&[0, 0, 0, 1, 0x41, 2]).is_h264_idr());
    }
}
