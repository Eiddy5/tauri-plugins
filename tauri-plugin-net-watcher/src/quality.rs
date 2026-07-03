mod probe {
    use std::time::Duration;

    use chrono::{DateTime, Utc};
    use tokio::{
        io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
        net::{lookup_host, TcpStream},
        time::{timeout, Instant},
    };
    use tokio_native_tls::TlsConnector;
    use url::{Host, Url};
    use uuid::Uuid;

    use crate::models::{
        HttpProbeResult, ProbeError, ProbePhases, ProbeResult, ProbeStatus, ProbeTarget,
        ProbeTargetType,
    };

    type ProbeFailure = (&'static str, String);

    pub(crate) struct HttpProber {
        timeout_ms: u64,
    }

    impl HttpProber {
        pub(crate) fn new(timeout_ms: u64) -> Self {
            Self { timeout_ms }
        }

        pub(crate) async fn probe(&self, target: &ProbeTarget) -> ProbeResult {
            let started_at = Utc::now();
            let started = Instant::now();
            let mut phases = ProbePhases::default();

            let outcome = timeout(
                Duration::from_millis(self.timeout_ms),
                self.probe_inner(target, &mut phases),
            )
            .await;

            let ended_at = Utc::now();
            let duration_ms = elapsed_ms(started);

            match outcome {
                Ok(Ok(status_code)) => {
                    success_probe(started_at, ended_at, duration_ms, phases, status_code)
                }
                Ok(Err((code, message))) => {
                    failed_probe(started_at, ended_at, duration_ms, phases, code, message)
                }
                Err(_) => failed_probe(
                    started_at,
                    ended_at,
                    duration_ms,
                    phases,
                    "http_timeout",
                    format!("probe timed out after {}ms", self.timeout_ms),
                ),
            }
        }

        async fn probe_inner(
            &self,
            target: &ProbeTarget,
            phases: &mut ProbePhases,
        ) -> Result<u16, ProbeFailure> {
            if target.target_type != ProbeTargetType::Http {
                return Err(("invalid_config", "target type must be http".to_string()));
            }

            let url =
                Url::parse(&target.url).map_err(|error| ("invalid_config", error.to_string()))?;
            let scheme = url.scheme();
            if scheme != "http" && scheme != "https" {
                return Err((
                    "invalid_config",
                    "target URL must use http or https".to_string(),
                ));
            }

            let host = url
                .host_str()
                .ok_or_else(|| ("invalid_config", "target host is missing".to_string()))?
                .to_string();
            let port = url
                .port_or_known_default()
                .ok_or_else(|| ("invalid_config", "target port is missing".to_string()))?;

            let dns_started = Instant::now();
            let mut addresses = lookup_host((host.as_str(), port))
                .await
                .map_err(|error| ("dns_failed", error.to_string()))?;
            phases.dns_ms = Some(elapsed_ms(dns_started));

            let address = addresses
                .next()
                .ok_or_else(|| ("dns_failed", "no address returned".to_string()))?;

            let tcp_started = Instant::now();
            let stream = TcpStream::connect(address)
                .await
                .map_err(|error| ("tcp_failed", error.to_string()))?;
            phases.tcp_ms = Some(elapsed_ms(tcp_started));

            if scheme == "https" {
                let tls_started = Instant::now();
                let connector = native_tls::TlsConnector::new()
                    .map(TlsConnector::from)
                    .map_err(|error| ("tls_failed", error.to_string()))?;
                let mut stream = connector
                    .connect(host.as_str(), stream)
                    .await
                    .map_err(|error| ("tls_failed", error.to_string()))?;
                phases.tls_ms = Some(elapsed_ms(tls_started));
                send_head_and_read_status(&mut stream, &url, phases).await
            } else {
                let mut stream = stream;
                send_head_and_read_status(&mut stream, &url, phases).await
            }
        }
    }

    fn success_probe(
        started_at: DateTime<Utc>,
        ended_at: DateTime<Utc>,
        duration_ms: u64,
        phases: ProbePhases,
        status_code: u16,
    ) -> ProbeResult {
        ProbeResult {
            id: format!("probe_{}", Uuid::new_v4()),
            status: ProbeStatus::Success,
            started_at,
            ended_at,
            duration_ms,
            phases,
            http: Some(HttpProbeResult { status_code }),
            error: None,
        }
    }

    fn failed_probe(
        started_at: DateTime<Utc>,
        ended_at: DateTime<Utc>,
        duration_ms: u64,
        phases: ProbePhases,
        code: &'static str,
        message: String,
    ) -> ProbeResult {
        ProbeResult {
            id: format!("probe_{}", Uuid::new_v4()),
            status: ProbeStatus::Failed,
            started_at,
            ended_at,
            duration_ms,
            phases,
            http: None,
            error: Some(ProbeError {
                code: code.to_string(),
                message,
            }),
        }
    }

    async fn send_head_and_read_status<S>(
        stream: &mut S,
        url: &Url,
        phases: &mut ProbePhases,
    ) -> Result<u16, ProbeFailure>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let http_started = Instant::now();
        let request = format!(
            "HEAD {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: tauri-plugin-net-watcher/0.1\r\n\r\n",
            request_target(url),
            host_header(url)?
        );

        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|error| ("http_failed", error.to_string()))?;
        stream
            .flush()
            .await
            .map_err(|error| ("http_failed", error.to_string()))?;

        let mut buffer = Vec::with_capacity(256);
        let mut chunk = [0_u8; 128];

        while !buffer.windows(2).any(|window| window == b"\r\n") && buffer.len() < 8 * 1024 {
            let read = stream
                .read(&mut chunk)
                .await
                .map_err(|error| ("http_failed", error.to_string()))?;
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }

        phases.http_ms = Some(elapsed_ms(http_started));

        let status_code = parse_status_code(&buffer)?;
        if (200..400).contains(&status_code) {
            Ok(status_code)
        } else {
            Err((
                "http_status_error",
                format!("unexpected HTTP status {status_code}"),
            ))
        }
    }

    fn request_target(url: &Url) -> String {
        let path = if url.path().is_empty() {
            "/"
        } else {
            url.path()
        };
        match url.query() {
            Some(query) => format!("{path}?{query}"),
            None => path.to_string(),
        }
    }

    fn host_header(url: &Url) -> Result<String, ProbeFailure> {
        let host = url
            .host()
            .ok_or_else(|| ("invalid_config", "target host is missing".to_string()))?;
        let host = match host {
            Host::Domain(domain) => domain.to_string(),
            Host::Ipv4(address) => address.to_string(),
            Host::Ipv6(address) => format!("[{address}]"),
        };

        match (url.port(), default_port_for_scheme(url.scheme())) {
            (Some(port), Some(default_port)) if port != default_port => {
                Ok(format!("{host}:{port}"))
            }
            (Some(port), None) => Ok(format!("{host}:{port}")),
            _ => Ok(host.to_string()),
        }
    }

    fn default_port_for_scheme(scheme: &str) -> Option<u16> {
        match scheme {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        }
    }

    fn parse_status_code(buffer: &[u8]) -> Result<u16, ProbeFailure> {
        let text = std::str::from_utf8(buffer)
            .map_err(|error| ("http_failed", format!("invalid HTTP response: {error}")))?;
        let status_line = text
            .lines()
            .next()
            .ok_or_else(|| ("http_failed", "empty HTTP response".to_string()))?;
        let status = status_line
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| ("http_failed", "missing HTTP status code".to_string()))?;

        status
            .parse::<u16>()
            .map_err(|error| ("http_failed", format!("invalid HTTP status code: {error}")))
    }

    fn elapsed_ms(started: Instant) -> u64 {
        started.elapsed().as_millis().min(u64::MAX as u128) as u64
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn invalid_scheme_returns_failed_probe() {
            let target = ProbeTarget {
                target_type: ProbeTargetType::Http,
                url: "file:///tmp/health".to_string(),
            };

            let result = HttpProber::new(500).probe(&target).await;

            assert_eq!(result.status, ProbeStatus::Failed);
            assert_eq!(result.error.unwrap().code, "invalid_config");
        }

        #[tokio::test]
        async fn unreachable_target_returns_failed_probe() {
            let target = ProbeTarget {
                target_type: ProbeTargetType::Http,
                url: "http://127.0.0.1:9/health".to_string(),
            };

            let result = HttpProber::new(200).probe(&target).await;

            assert_eq!(result.status, ProbeStatus::Failed);
            assert!(result.duration_ms <= 1_000);
            assert!(result.error.is_some());
        }

        #[test]
        fn host_header_brackets_default_port_ipv6_literal() {
            let url = Url::parse("http://[::1]/health").unwrap();

            let header = host_header(&url).unwrap();

            assert_eq!(header, "[::1]");
        }

        #[test]
        fn host_header_brackets_explicit_non_default_port_ipv6_literal() {
            let url = Url::parse("http://[::1]:8080/health").unwrap();

            let header = host_header(&url).unwrap();

            assert_eq!(header, "[::1]:8080");
        }

        #[test]
        fn host_header_includes_explicit_non_default_port_for_domain() {
            let url = Url::parse("http://example.com:8080/health").unwrap();

            let header = host_header(&url).unwrap();

            assert_eq!(header, "example.com:8080");
        }
    }
}

mod stats {
    use std::collections::VecDeque;

    use crate::models::{LatencySummary, ProbeResult, ProbeStatus, QualitySummary};

    #[derive(Debug, Clone)]
    pub(crate) struct RollingWindow {
        capacity: usize,
        samples: VecDeque<ProbeResult>,
    }

    impl RollingWindow {
        pub(crate) fn new(capacity: usize) -> Self {
            let capacity = capacity.max(1);

            Self {
                capacity,
                samples: VecDeque::with_capacity(capacity),
            }
        }

        pub(crate) fn push(&mut self, sample: ProbeResult) {
            if self.samples.len() == self.capacity {
                self.samples.pop_front();
            }

            self.samples.push_back(sample);
        }

        pub(crate) fn latest(&self) -> Option<ProbeResult> {
            self.samples.back().cloned()
        }

        pub(crate) fn summary(&self) -> QualitySummary {
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
}

mod state {
    use crate::models::{
        NetworkLayerState, OverallState, QualityLayerState, QualitySummary, SnapshotState,
    };

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub(crate) struct StateConfig {
        pub(crate) degraded_failure_rate: f64,
        pub(crate) degraded_p95_latency_ms: u64,
        pub(crate) offline_consecutive_failures: usize,
    }

    pub(crate) fn evaluate_state(
        has_available_interface: bool,
        summary: &QualitySummary,
        config: &StateConfig,
    ) -> SnapshotState {
        let config = normalize_config(config);

        if !has_available_interface {
            return SnapshotState {
                overall: OverallState::Offline,
                network: NetworkLayerState::Disconnected,
                quality: QualityLayerState::Unknown,
                score: 0,
                reason: "no_available_interface".to_string(),
            };
        }

        if summary.sample_count == 0 {
            return SnapshotState {
                overall: OverallState::Unknown,
                network: NetworkLayerState::Connected,
                quality: QualityLayerState::Unknown,
                score: 0,
                reason: "insufficient_data".to_string(),
            };
        }

        if summary.consecutive_failures >= config.offline_consecutive_failures {
            return SnapshotState {
                overall: OverallState::LocalOnly,
                network: NetworkLayerState::Connected,
                quality: QualityLayerState::Unreachable,
                score: 10,
                reason: "target_unreachable".to_string(),
            };
        }

        let score = calculate_score(summary);
        let is_degraded = summary.failure_rate >= config.degraded_failure_rate
            || summary.latency_ms.p95 >= config.degraded_p95_latency_ms
            || summary.jitter_ms >= 300;

        if is_degraded {
            SnapshotState {
                overall: OverallState::Degraded,
                network: NetworkLayerState::Connected,
                quality: QualityLayerState::Unstable,
                score,
                reason: "high_latency_or_recent_failures".to_string(),
            }
        } else {
            SnapshotState {
                overall: OverallState::Online,
                network: NetworkLayerState::Connected,
                quality: QualityLayerState::Stable,
                score,
                reason: "network_stable".to_string(),
            }
        }
    }

    fn normalize_config(config: &StateConfig) -> StateConfig {
        StateConfig {
            degraded_failure_rate: if config.degraded_failure_rate <= 0.0 {
                0.15
            } else {
                config.degraded_failure_rate.min(1.0)
            },
            degraded_p95_latency_ms: if config.degraded_p95_latency_ms == 0 {
                800
            } else {
                config.degraded_p95_latency_ms
            },
            offline_consecutive_failures: config.offline_consecutive_failures.max(1),
        }
    }

    fn calculate_score(summary: &QualitySummary) -> u8 {
        let failure_penalty = (summary.failure_rate.clamp(0.0, 1.0) * 60.0).round() as i32;
        let latency_penalty = (summary.latency_ms.p95 / 100).min(30) as i32;
        let jitter_penalty = (summary.jitter_ms / 25).min(20) as i32;
        let consecutive_penalty = summary.consecutive_failures.saturating_mul(10).min(30) as i32;

        (100 - failure_penalty - latency_penalty - jitter_penalty - consecutive_penalty)
            .clamp(0, 100) as u8
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::models::*;

        fn config() -> StateConfig {
            StateConfig {
                degraded_failure_rate: 0.15,
                degraded_p95_latency_ms: 800,
                offline_consecutive_failures: 3,
            }
        }

        #[test]
        fn no_interface_is_offline() {
            let state = evaluate_state(false, &QualitySummary::default(), &config());

            assert_eq!(state.overall, OverallState::Offline);
            assert_eq!(state.network, NetworkLayerState::Disconnected);
            assert_eq!(state.reason, "no_available_interface");
        }

        #[test]
        fn consecutive_failures_are_local_only() {
            let summary = QualitySummary {
                sample_count: 3,
                failure_count: 3,
                failure_rate: 1.0,
                consecutive_failures: 3,
                ..Default::default()
            };

            let state = evaluate_state(true, &summary, &config());

            assert_eq!(state.overall, OverallState::LocalOnly);
            assert_eq!(state.quality, QualityLayerState::Unreachable);
        }

        #[test]
        fn high_latency_is_degraded() {
            let summary = QualitySummary {
                sample_count: 20,
                success_count: 20,
                latency_ms: LatencySummary {
                    avg: 400,
                    min: 100,
                    max: 1200,
                    p95: 900,
                },
                ..Default::default()
            };

            let state = evaluate_state(true, &summary, &config());

            assert_eq!(state.overall, OverallState::Degraded);
            assert_eq!(state.quality, QualityLayerState::Unstable);
        }

        #[test]
        fn stable_summary_is_online() {
            let summary = QualitySummary {
                sample_count: 20,
                success_count: 20,
                latency_ms: LatencySummary {
                    avg: 100,
                    min: 80,
                    max: 160,
                    p95: 150,
                },
                jitter_ms: 20,
                ..Default::default()
            };

            let state = evaluate_state(true, &summary, &config());

            assert_eq!(state.overall, OverallState::Online);
            assert_eq!(state.quality, QualityLayerState::Stable);
            assert!(state.score > 80);
        }

        #[test]
        fn extreme_consecutive_failures_do_not_overflow_score() {
            let summary = QualitySummary {
                sample_count: 20,
                failure_count: 20,
                failure_rate: 1.0,
                latency_ms: LatencySummary {
                    avg: u64::MAX,
                    min: u64::MAX,
                    max: u64::MAX,
                    p95: u64::MAX,
                },
                jitter_ms: u64::MAX,
                consecutive_failures: usize::MAX,
                ..Default::default()
            };

            let score = calculate_score(&summary);

            assert_eq!(score, 0);
        }

        #[test]
        fn zero_thresholds_do_not_degrade_healthy_summary() {
            let summary = QualitySummary {
                sample_count: 20,
                success_count: 20,
                latency_ms: LatencySummary {
                    avg: 100,
                    min: 80,
                    max: 160,
                    p95: 150,
                },
                jitter_ms: 20,
                ..Default::default()
            };
            let config = StateConfig {
                degraded_failure_rate: 0.0,
                degraded_p95_latency_ms: 0,
                offline_consecutive_failures: 0,
            };

            let state = evaluate_state(true, &summary, &config);

            assert_eq!(state.overall, OverallState::Online);
            assert_eq!(state.quality, QualityLayerState::Stable);
        }
    }
}

pub(crate) use probe::HttpProber;
pub(crate) use state::{evaluate_state, StateConfig};
pub(crate) use stats::RollingWindow;
