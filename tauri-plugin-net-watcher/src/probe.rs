use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{lookup_host, TcpStream},
    time::{timeout, Instant},
};
use tokio_native_tls::TlsConnector;
use url::Url;
use uuid::Uuid;

use crate::models::{
    HttpProbeResult, ProbeError, ProbePhases, ProbeResult, ProbeStatus, ProbeTarget,
    ProbeTargetType,
};

type ProbeFailure = (&'static str, String);

pub struct HttpProber {
    timeout_ms: u64,
}

impl HttpProber {
    pub fn new(timeout_ms: u64) -> Self {
        Self { timeout_ms }
    }

    pub async fn probe(&self, target: &ProbeTarget) -> ProbeResult {
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
            Ok(Ok(status_code)) => success_probe(started_at, ended_at, duration_ms, phases, status_code),
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

        let url = Url::parse(&target.url).map_err(|error| ("invalid_config", error.to_string()))?;
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
    let path = if url.path().is_empty() { "/" } else { url.path() };
    match url.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    }
}

fn host_header(url: &Url) -> Result<String, ProbeFailure> {
    let host = url
        .host_str()
        .ok_or_else(|| ("invalid_config", "target host is missing".to_string()))?;
    let default_port = url.port_or_known_default();

    match (url.port(), default_port) {
        (Some(port), Some(default)) if port != default => Ok(format!("{host}:{port}")),
        (Some(port), None) => Ok(format!("{host}:{port}")),
        _ => Ok(host.to_string()),
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
}
