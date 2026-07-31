use std::time::Instant;

use crate::{
    models::{QualityMode, QualityOptions},
    platform::windows::media::recommended_screen_share_bitrate,
    webrtc::signaling::BandwidthEstimate,
};

use super::{
    content::{ContentActivityTracker, ContentState},
    network::NetworkEstimator,
};

#[derive(Debug, Clone)]
pub(crate) struct AdaptationDecision {
    pub(crate) mode: QualityMode,
    pub(crate) content_state: &'static str,
    pub(crate) target_fps: u32,
    pub(crate) target_bitrate_bps: u64,
    pub(crate) network_estimate_bps: Option<u64>,
    pub(crate) reason: &'static str,
}

pub(crate) struct AdaptiveQualityController {
    options: QualityOptions,
    requested_fps: u32,
    maximum_bitrate_bps: u64,
    content: ContentActivityTracker,
    network: NetworkEstimator,
}

impl AdaptiveQualityController {
    pub(crate) fn new(
        width: u32,
        height: u32,
        requested_fps: u32,
        options: QualityOptions,
        now: Instant,
    ) -> Self {
        Self {
            options,
            requested_fps: requested_fps.max(1),
            maximum_bitrate_bps: u64::from(recommended_screen_share_bitrate(
                width,
                height,
                requested_fps,
            )),
            content: ContentActivityTracker::default(),
            network: NetworkEstimator::new(now),
        }
    }

    pub(crate) fn update_network_feedback(&mut self, estimate: Option<BandwidthEstimate>) {
        self.network.observe(estimate);
    }

    pub(crate) fn observe_frame(&mut self, fresh: bool, now: Instant) {
        if self.options.mode == QualityMode::Auto {
            self.content.observe(fresh, self.requested_fps, now);
        }
    }

    pub(crate) fn decision(&self, now: Instant) -> AdaptationDecision {
        let content = match self.options.mode {
            QualityMode::Clarity => ContentState::Static,
            QualityMode::Motion => ContentState::Motion,
            QualityMode::Auto => self.content.state(),
            QualityMode::Fixed => ContentState::Interactive,
        };
        let minimum_fps = self
            .options
            .minimum_fps
            .unwrap_or(1)
            .clamp(1, self.requested_fps);
        let target_fps = match self.options.mode {
            QualityMode::Fixed | QualityMode::Motion => self.requested_fps,
            QualityMode::Clarity => self.requested_fps.min(15).max(minimum_fps),
            QualityMode::Auto => match content {
                ContentState::Static => self.requested_fps.min(15).max(minimum_fps),
                ContentState::Interactive => self.requested_fps.min(30).max(minimum_fps),
                ContentState::Motion => self.requested_fps,
            },
        };

        let ratio = match self.options.mode {
            QualityMode::Clarity => 85,
            QualityMode::Motion | QualityMode::Fixed => 50,
            QualityMode::Auto => match content {
                ContentState::Static => 75,
                ContentState::Interactive => 70,
                ContentState::Motion => 65,
            },
        };
        let configured_minimum = self
            .options
            .minimum_bitrate_kbps
            .map(|kbps| u64::from(kbps).saturating_mul(1_000))
            .unwrap_or(0);
        let minimum_bitrate_bps = self
            .maximum_bitrate_bps
            .saturating_mul(ratio)
            .div_ceil(100)
            .max(configured_minimum)
            .min(self.maximum_bitrate_bps);
        let network_estimate_bps = self.network.usable_bitrate_bps(now);
        let target_bitrate_bps = network_estimate_bps
            .map(|estimate| estimate.saturating_mul(85).div_ceil(100))
            .unwrap_or(self.maximum_bitrate_bps)
            .clamp(minimum_bitrate_bps, self.maximum_bitrate_bps);
        let reason = if network_estimate_bps.is_none() {
            "network-warmup-or-stale"
        } else if target_bitrate_bps == minimum_bitrate_bps {
            "quality-floor"
        } else if target_bitrate_bps == self.maximum_bitrate_bps {
            "profile-maximum"
        } else {
            "network-estimate"
        };

        AdaptationDecision {
            mode: self.options.mode,
            content_state: if self.options.mode == QualityMode::Fixed {
                "fixed"
            } else {
                content.label()
            },
            target_fps,
            target_bitrate_bps,
            network_estimate_bps,
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn startup_remb_does_not_immediately_reduce_quality() {
        let now = Instant::now();
        let mut controller =
            AdaptiveQualityController::new(1920, 1080, 60, QualityOptions::default(), now);
        controller.update_network_feedback(Some(BandwidthEstimate {
            bitrate_bps: 138_000,
            received_at: now,
        }));
        assert_eq!(
            controller
                .decision(now + Duration::from_secs(1))
                .target_bitrate_bps,
            8_000_000
        );
    }

    #[test]
    fn stale_remb_recovers_to_profile_maximum() {
        let now = Instant::now();
        let mut controller =
            AdaptiveQualityController::new(1920, 1080, 60, QualityOptions::default(), now);
        controller.update_network_feedback(Some(BandwidthEstimate {
            bitrate_bps: 4_800_000,
            received_at: now + Duration::from_secs(3),
        }));
        assert_eq!(
            controller
                .decision(now + Duration::from_secs(4))
                .target_bitrate_bps,
            4_080_000
        );
        assert_eq!(
            controller
                .decision(now + Duration::from_secs(7))
                .target_bitrate_bps,
            8_000_000
        );
    }
}
