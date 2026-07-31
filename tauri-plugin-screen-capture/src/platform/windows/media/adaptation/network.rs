use std::time::{Duration, Instant};

use crate::webrtc::signaling::BandwidthEstimate;

const STARTUP_WARMUP: Duration = Duration::from_secs(3);
const ESTIMATE_MAX_AGE: Duration = Duration::from_secs(3);
const EMA_NEW_SAMPLE_WEIGHT: u64 = 25;

pub(crate) struct NetworkEstimator {
    started_at: Instant,
    smoothed_bitrate_bps: Option<u64>,
    last_sample_at: Option<Instant>,
}

impl NetworkEstimator {
    pub(crate) fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            smoothed_bitrate_bps: None,
            last_sample_at: None,
        }
    }

    pub(crate) fn observe(&mut self, estimate: Option<BandwidthEstimate>) {
        let Some(estimate) = estimate else {
            return;
        };
        if self
            .last_sample_at
            .is_some_and(|seen| estimate.received_at <= seen)
        {
            return;
        }
        self.smoothed_bitrate_bps = Some(match self.smoothed_bitrate_bps {
            None => estimate.bitrate_bps,
            Some(current) => current
                .saturating_mul(100 - EMA_NEW_SAMPLE_WEIGHT)
                .saturating_add(estimate.bitrate_bps.saturating_mul(EMA_NEW_SAMPLE_WEIGHT))
                .div_ceil(100),
        });
        self.last_sample_at = Some(estimate.received_at);
    }

    pub(crate) fn usable_bitrate_bps(&self, now: Instant) -> Option<u64> {
        if now.duration_since(self.started_at) < STARTUP_WARMUP {
            return None;
        }
        let received_at = self.last_sample_at?;
        if now.duration_since(received_at) > ESTIMATE_MAX_AGE {
            return None;
        }
        self.smoothed_bitrate_bps
    }
}
