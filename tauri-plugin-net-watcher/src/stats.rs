use std::collections::VecDeque;

use crate::models::{LatencySummary, ProbeResult, ProbeStatus, QualitySummary};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RollingWindow {
    capacity: usize,
    samples: VecDeque<ProbeResult>,
}

#[allow(dead_code)]
impl RollingWindow {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);

        Self {
            capacity,
            samples: VecDeque::with_capacity(capacity),
        }
    }

    pub fn push(&mut self, sample: ProbeResult) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }

        self.samples.push_back(sample);
    }

    pub fn latest(&self) -> Option<ProbeResult> {
        self.samples.back().cloned()
    }

    pub fn summary(&self) -> QualitySummary {
        let sample_count = self.samples.len();
        let success_count = self
            .samples
            .iter()
            .filter(|sample| sample.status == ProbeStatus::Success)
            .count();
        let failure_count = sample_count - success_count;
        let failure_rate = if sample_count == 0 {
            0.0
        } else {
            failure_count as f64 / sample_count as f64
        };

        let successful_latencies = self
            .samples
            .iter()
            .filter(|sample| sample.status == ProbeStatus::Success)
            .map(|sample| sample.duration_ms)
            .collect::<Vec<_>>();

        let latency_ms = latency_summary(&successful_latencies);
        let jitter_ms = jitter(&successful_latencies);
        let consecutive_failures = self
            .samples
            .iter()
            .rev()
            .take_while(|sample| sample.status == ProbeStatus::Failed)
            .count();
        let last_success_at = self
            .samples
            .iter()
            .rev()
            .find(|sample| sample.status == ProbeStatus::Success)
            .map(|sample| sample.ended_at);
        let last_failure = self
            .samples
            .iter()
            .rev()
            .find(|sample| sample.status == ProbeStatus::Failed);
        let last_failure_at = last_failure.map(|sample| sample.ended_at);
        let last_failure_reason = last_failure
            .and_then(|sample| sample.error.as_ref())
            .map(|error| error.code.clone());

        QualitySummary {
            sample_count,
            success_count,
            failure_count,
            failure_rate,
            latency_ms,
            jitter_ms,
            consecutive_failures,
            last_success_at,
            last_failure_at,
            last_failure_reason,
        }
    }
}

#[allow(dead_code)]
fn latency_summary(latencies: &[u64]) -> LatencySummary {
    if latencies.is_empty() {
        return LatencySummary::default();
    }

    let avg = clamp_u128_to_u64(
        latencies
            .iter()
            .map(|latency| *latency as u128)
            .sum::<u128>()
            / latencies.len() as u128,
    );
    let min = latencies.iter().copied().min().unwrap_or_default();
    let max = latencies.iter().copied().max().unwrap_or_default();
    let mut sorted = latencies.to_vec();
    sorted.sort_unstable();
    let p95_index = ((sorted.len() * 95).div_ceil(100)).saturating_sub(1);
    let p95 = sorted[p95_index];

    LatencySummary { avg, min, max, p95 }
}

#[allow(dead_code)]
fn jitter(latencies: &[u64]) -> u64 {
    if latencies.len() < 2 {
        return 0;
    }

    let total_delta = latencies
        .windows(2)
        .map(|pair| pair[0].abs_diff(pair[1]) as u128)
        .sum::<u128>();

    clamp_u128_to_u64(total_delta / (latencies.len() - 1) as u128)
}

#[allow(dead_code)]
fn clamp_u128_to_u64(value: u128) -> u64 {
    value.min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;
    use chrono::Utc;

    fn probe(status: ProbeStatus, duration_ms: u64, reason: Option<&str>) -> ProbeResult {
        ProbeResult {
            id: format!("probe_{duration_ms}"),
            status,
            started_at: Utc::now(),
            ended_at: Utc::now(),
            duration_ms,
            phases: ProbePhases::default(),
            http: None,
            error: reason.map(|code| ProbeError {
                code: code.to_string(),
                message: code.to_string(),
            }),
        }
    }

    #[test]
    fn rolling_window_computes_failure_rate_latency_and_consecutive_failures() {
        let mut window = RollingWindow::new(4);

        window.push(probe(ProbeStatus::Success, 100, None));
        window.push(probe(ProbeStatus::Failed, 300, Some("tcp_timeout")));
        window.push(probe(ProbeStatus::Success, 200, None));
        window.push(probe(ProbeStatus::Failed, 400, Some("http_timeout")));

        let summary = window.summary();

        assert_eq!(summary.sample_count, 4);
        assert_eq!(summary.success_count, 2);
        assert_eq!(summary.failure_count, 2);
        assert_eq!(summary.failure_rate, 0.5);
        assert_eq!(summary.latency_ms.avg, 150);
        assert_eq!(summary.latency_ms.min, 100);
        assert_eq!(summary.latency_ms.max, 200);
        assert_eq!(summary.consecutive_failures, 1);
        assert_eq!(summary.last_failure_reason.as_deref(), Some("http_timeout"));
    }

    #[test]
    fn rolling_window_drops_old_samples() {
        let mut window = RollingWindow::new(2);

        window.push(probe(ProbeStatus::Success, 100, None));
        window.push(probe(ProbeStatus::Success, 200, None));
        window.push(probe(ProbeStatus::Success, 300, None));

        let summary = window.summary();

        assert_eq!(summary.sample_count, 2);
        assert_eq!(summary.latency_ms.min, 200);
        assert_eq!(summary.latency_ms.max, 300);
    }

    #[test]
    fn rolling_window_handles_large_latency_sum_without_overflow() {
        let mut window = RollingWindow::new(3);

        window.push(probe(ProbeStatus::Success, u64::MAX, None));
        window.push(probe(ProbeStatus::Success, u64::MAX, None));
        window.push(probe(ProbeStatus::Success, u64::MAX, None));

        let summary = window.summary();

        assert_eq!(summary.latency_ms.avg, u64::MAX);
        assert_eq!(summary.latency_ms.min, u64::MAX);
        assert_eq!(summary.latency_ms.max, u64::MAX);
        assert_eq!(summary.latency_ms.p95, u64::MAX);
    }

    #[test]
    fn rolling_window_handles_large_jitter_sum_without_overflow() {
        let mut window = RollingWindow::new(3);

        window.push(probe(ProbeStatus::Success, 0, None));
        window.push(probe(ProbeStatus::Success, u64::MAX, None));
        window.push(probe(ProbeStatus::Success, 0, None));

        let summary = window.summary();

        assert_eq!(summary.jitter_ms, u64::MAX);
    }
}
