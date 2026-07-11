use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

use crate::{ProbeStatus, TargetQualityState};

const FAST_FAILURE_INTERVAL: Duration = Duration::from_secs(3);
const UNSTABLE_QUALITY_INTERVAL: Duration = Duration::from_secs(5);
const RECOVERY_CONFIRM_INTERVAL: Duration = Duration::from_secs(3);
const FAILURE_BACKOFF: [Duration; 4] = [
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(40),
    Duration::from_secs(60),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetScheduleMode {
    Verifying,
    Healthy,
    Suspect,
    Unavailable,
    Recovering,
    Suspended,
}

pub(crate) struct TargetSchedule {
    mode: TargetScheduleMode,
    next_probe_at: Instant,
    consecutive_failures: usize,
    consecutive_recovery_successes: usize,
    probe_sequence: u64,
}

impl TargetSchedule {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            mode: TargetScheduleMode::Verifying,
            next_probe_at: now,
            consecutive_failures: 0,
            consecutive_recovery_successes: 0,
            probe_sequence: 0,
        }
    }

    pub(crate) fn reset(&mut self, now: Instant) {
        *self = Self::new(now);
    }

    pub(crate) fn suspend(&mut self) {
        self.mode = TargetScheduleMode::Suspended;
        self.consecutive_failures = 0;
        self.consecutive_recovery_successes = 0;
    }

    pub(crate) fn is_suspended(&self) -> bool {
        self.mode == TargetScheduleMode::Suspended
    }

    pub(crate) fn is_due(&self, now: Instant) -> bool {
        !self.is_suspended() && now >= self.next_probe_at
    }

    pub(crate) fn next_check_in(&self, now: Instant) -> Option<Duration> {
        (!self.is_suspended()).then(|| self.next_probe_at.saturating_duration_since(now))
    }

    pub(crate) fn record_result(
        &mut self,
        target_id: &str,
        status: ProbeStatus,
        quality: TargetQualityState,
        base_interval: Duration,
        now: Instant,
    ) {
        self.probe_sequence = self.probe_sequence.saturating_add(1);
        let interval = match status {
            ProbeStatus::Failed => self.record_failure(base_interval),
            ProbeStatus::Success => self.record_success(target_id, quality, base_interval),
        };
        self.next_probe_at = now + interval;
    }

    fn record_failure(&mut self, base_interval: Duration) -> Duration {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.consecutive_recovery_successes = 0;

        if self.consecutive_failures < 3 {
            self.mode = TargetScheduleMode::Suspect;
            return FAST_FAILURE_INTERVAL.min(base_interval);
        }

        self.mode = TargetScheduleMode::Unavailable;
        FAILURE_BACKOFF[(self.consecutive_failures - 3).min(FAILURE_BACKOFF.len() - 1)]
    }

    fn record_success(
        &mut self,
        target_id: &str,
        quality: TargetQualityState,
        base_interval: Duration,
    ) -> Duration {
        let recovering = self.consecutive_failures > 0
            || matches!(
                self.mode,
                TargetScheduleMode::Unavailable | TargetScheduleMode::Recovering
            );
        self.consecutive_failures = 0;

        if quality == TargetQualityState::Unstable {
            self.mode = TargetScheduleMode::Suspect;
            self.consecutive_recovery_successes = 0;
            return UNSTABLE_QUALITY_INTERVAL.min(base_interval);
        }

        if recovering && self.consecutive_recovery_successes == 0 {
            self.mode = TargetScheduleMode::Recovering;
            self.consecutive_recovery_successes = 1;
            return RECOVERY_CONFIRM_INTERVAL.min(base_interval);
        }

        self.mode = TargetScheduleMode::Healthy;
        self.consecutive_recovery_successes = 0;
        jittered_interval(base_interval, target_id, self.probe_sequence)
    }
}

fn jittered_interval(base: Duration, target_id: &str, sequence: u64) -> Duration {
    let mut hasher = DefaultHasher::new();
    target_id.hash(&mut hasher);
    sequence.hash(&mut hasher);
    let percent = 90 + hasher.finish() % 21;
    let millis = base
        .as_millis()
        .saturating_mul(percent as u128)
        .saturating_div(100)
        .min(u64::MAX as u128) as u64;

    Duration::from_millis(millis.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_schedule_moves_from_fast_confirmation_to_backoff() {
        let now = Instant::now();
        let base = Duration::from_secs(30);
        let mut schedule = TargetSchedule::new(now);

        schedule.record_result(
            "api",
            ProbeStatus::Failed,
            TargetQualityState::Unstable,
            base,
            now,
        );
        assert_eq!(schedule.mode, TargetScheduleMode::Suspect);
        assert_eq!(schedule.next_check_in(now), Some(Duration::from_secs(3)));

        schedule.record_result(
            "api",
            ProbeStatus::Failed,
            TargetQualityState::Unstable,
            base,
            now,
        );
        schedule.record_result(
            "api",
            ProbeStatus::Failed,
            TargetQualityState::Unstable,
            base,
            now,
        );
        assert_eq!(schedule.mode, TargetScheduleMode::Unavailable);
        assert_eq!(schedule.next_check_in(now), Some(Duration::from_secs(10)));

        schedule.record_result(
            "api",
            ProbeStatus::Failed,
            TargetQualityState::Unstable,
            base,
            now,
        );
        assert_eq!(schedule.next_check_in(now), Some(Duration::from_secs(20)));
    }

    #[test]
    fn failed_target_requires_one_fast_recovery_confirmation() {
        let now = Instant::now();
        let base = Duration::from_secs(30);
        let mut schedule = TargetSchedule::new(now);
        schedule.record_result(
            "api",
            ProbeStatus::Failed,
            TargetQualityState::Unstable,
            base,
            now,
        );

        schedule.record_result(
            "api",
            ProbeStatus::Success,
            TargetQualityState::Stable,
            base,
            now,
        );
        assert_eq!(schedule.mode, TargetScheduleMode::Recovering);
        assert_eq!(schedule.next_check_in(now), Some(Duration::from_secs(3)));

        schedule.record_result(
            "api",
            ProbeStatus::Success,
            TargetQualityState::Stable,
            base,
            now,
        );
        assert_eq!(schedule.mode, TargetScheduleMode::Healthy);
        let next = schedule.next_check_in(now).unwrap();
        assert!((Duration::from_secs(27)..=Duration::from_secs(33)).contains(&next));
    }

    #[test]
    fn reset_is_immediately_due_and_clears_suspension() {
        let now = Instant::now();
        let mut schedule = TargetSchedule::new(now);
        schedule.suspend();
        assert!(schedule.next_check_in(now).is_none());

        schedule.reset(now);

        assert!(schedule.is_due(now));
        assert_eq!(schedule.mode, TargetScheduleMode::Verifying);
    }
}
