use std::time::{Duration, Instant};

use chrono::Utc;

use crate::{
    InternetHintLevel, InternetHintSource, InternetProbeResult, InternetProbeStatus,
    InternetSnapshot, InternetStatus, ProbeError,
};

const INTERNET_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const INTERNET_RECOVERY_INTERVAL: Duration = Duration::from_secs(3);
const INTERNET_FAILURE_THRESHOLD: usize = 3;
const INTERNET_FAILURE_BACKOFF: [Duration; 4] = [
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(40),
    Duration::from_secs(60),
];

pub(crate) struct InternetMonitor {
    last_checked: Option<Instant>,
    snapshot: InternetSnapshot,
}

impl InternetMonitor {
    pub(crate) fn new() -> Self {
        Self {
            last_checked: None,
            snapshot: InternetSnapshot::default(),
        }
    }

    pub(crate) async fn refresh(
        &mut self,
        interface_available: Option<bool>,
        force: bool,
    ) -> InternetSnapshot {
        match interface_available {
            None => {
                self.snapshot = unknown_snapshot("network_snapshot_unavailable");
                self.last_checked = None;
                return self.snapshot.clone();
            }
            Some(false) => {
                self.snapshot = unavailable_without_interface();
                self.last_checked = None;
                return self.snapshot.clone();
            }
            Some(true) => {}
        }

        let is_due = self.is_due();
        if !force && !is_due {
            return self.snapshot.clone();
        }

        let previous = self.snapshot.clone();
        let next = check_platform_internet().await;
        self.last_checked = Some(Instant::now());

        self.snapshot = apply_failure_policy(&previous, next);
        self.snapshot.clone()
    }

    pub(crate) fn reset_for_network_change(&mut self) {
        self.last_checked = None;
        self.snapshot = InternetSnapshot::default();
    }

    pub(crate) fn next_check_in(&self, interface_available: Option<bool>) -> Duration {
        if interface_available != Some(true) {
            return INTERNET_CHECK_INTERVAL;
        }

        let interval = self.check_interval();

        self.last_checked
            .map(|checked| interval.saturating_sub(checked.elapsed()))
            .unwrap_or(Duration::ZERO)
    }

    fn check_interval(&self) -> Duration {
        match self.snapshot.status {
            InternetStatus::Available | InternetStatus::CaptivePortal => INTERNET_CHECK_INTERVAL,
            InternetStatus::Unavailable
                if self.snapshot.consecutive_failures >= INTERNET_FAILURE_THRESHOLD =>
            {
                INTERNET_FAILURE_BACKOFF[(self.snapshot.consecutive_failures
                    - INTERNET_FAILURE_THRESHOLD)
                    .min(INTERNET_FAILURE_BACKOFF.len() - 1)]
            }
            InternetStatus::Unknown | InternetStatus::Degraded | InternetStatus::Unavailable => {
                INTERNET_RECOVERY_INTERVAL
            }
        }
    }

    fn is_due(&self) -> bool {
        match self.last_checked {
            Some(checked) => checked.elapsed() >= self.check_interval(),
            None => true,
        }
    }
}

fn apply_failure_policy(
    previous: &InternetSnapshot,
    mut next: InternetSnapshot,
) -> InternetSnapshot {
    if next.active_probe.is_none() && next.system_hint.source == InternetHintSource::Unavailable {
        next.consecutive_failures = 0;
        return next;
    }

    if next.verified {
        next.consecutive_failures = 0;
        return next;
    }

    next.consecutive_failures = previous.consecutive_failures.saturating_add(1);
    if next.status == InternetStatus::CaptivePortal {
        return next;
    }

    if next.consecutive_failures >= INTERNET_FAILURE_THRESHOLD {
        next.status = InternetStatus::Unavailable;
        next.reason = "internet_probe_failure_threshold_reached".to_string();
        return next;
    }

    let was_available = matches!(
        previous.status,
        InternetStatus::Available | InternetStatus::Degraded
    );
    if was_available {
        next.status = InternetStatus::Degraded;
        next.reason = "internet_reverification_failed".to_string();
    }

    next
}

fn unknown_snapshot(reason: &str) -> InternetSnapshot {
    InternetSnapshot {
        reason: reason.to_string(),
        checked_at: Some(Utc::now()),
        ..InternetSnapshot::default()
    }
}

fn unavailable_without_interface() -> InternetSnapshot {
    InternetSnapshot {
        status: InternetStatus::Unavailable,
        checked_at: Some(Utc::now()),
        reason: "no_available_interface".to_string(),
        ..InternetSnapshot::default()
    }
}

fn classify_platform_evidence(
    hint_source: InternetHintSource,
    hint_level: InternetHintLevel,
    active_probe: InternetProbeResult,
    captive_portal: bool,
) -> InternetSnapshot {
    let (status, verified, reason) = match active_probe.status {
        InternetProbeStatus::Success => {
            (InternetStatus::Available, true, "internet_probe_succeeded")
        }
        InternetProbeStatus::UnexpectedResponse if captive_portal => (
            InternetStatus::CaptivePortal,
            false,
            "captive_portal_detected",
        ),
        InternetProbeStatus::UnexpectedResponse => (
            InternetStatus::Unavailable,
            false,
            "internet_unexpected_response",
        ),
        InternetProbeStatus::Failed
            if matches!(
                hint_level,
                InternetHintLevel::InternetAccess | InternetHintLevel::ConstrainedInternetAccess
            ) =>
        {
            (
                InternetStatus::Degraded,
                false,
                "system_hint_available_but_active_probe_failed",
            )
        }
        InternetProbeStatus::Failed => {
            (InternetStatus::Unavailable, false, "internet_probe_failed")
        }
    };

    InternetSnapshot {
        status,
        verified,
        system_hint: crate::InternetSystemHint {
            source: hint_source,
            level: hint_level,
        },
        active_probe: Some(active_probe),
        captive_portal,
        checked_at: Some(Utc::now()),
        consecutive_failures: 0,
        reason: reason.to_string(),
    }
}

#[cfg(target_os = "windows")]
async fn check_platform_internet() -> InternetSnapshot {
    let hint_level = windows_ncsi_hint();
    let (active_probe, captive_portal) = windows_active_probe().await;
    classify_platform_evidence(
        InternetHintSource::WindowsNcsi,
        hint_level,
        active_probe,
        captive_portal,
    )
}

#[cfg(target_os = "macos")]
async fn check_platform_internet() -> InternetSnapshot {
    let hint_level = macos_reachability_hint();
    let (active_probe, captive_portal) = macos_active_probe().await;
    classify_platform_evidence(
        InternetHintSource::MacosReachability,
        hint_level,
        active_probe,
        captive_portal,
    )
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
async fn check_platform_internet() -> InternetSnapshot {
    unknown_snapshot("internet_check_unsupported")
}

#[cfg(target_os = "macos")]
fn macos_reachability_hint() -> InternetHintLevel {
    use std::ffi::CString;
    use system_configuration::network_reachability::{ReachabilityFlags, SCNetworkReachability};

    let Ok(host) = CString::new("captive.apple.com") else {
        return InternetHintLevel::Unknown;
    };
    let Some(reachability) = SCNetworkReachability::from_host(&host) else {
        return InternetHintLevel::Unknown;
    };
    let Ok(flags) = reachability.reachability() else {
        return InternetHintLevel::Unknown;
    };

    if !flags.contains(ReachabilityFlags::REACHABLE) {
        InternetHintLevel::None
    } else if flags.contains(ReachabilityFlags::INTERVENTION_REQUIRED) {
        InternetHintLevel::ConstrainedInternetAccess
    } else if flags.contains(ReachabilityFlags::CONNECTION_REQUIRED)
        && !flags.intersects(
            ReachabilityFlags::CONNECTION_ON_DEMAND | ReachabilityFlags::CONNECTION_ON_TRAFFIC,
        )
    {
        InternetHintLevel::LocalAccess
    } else {
        InternetHintLevel::InternetAccess
    }
}

#[cfg(target_os = "macos")]
async fn macos_active_probe() -> (InternetProbeResult, bool) {
    use std::{ffi::c_void, ptr::NonNull, sync::Mutex};

    use block2::RcBlock;
    use objc2_foundation::{
        NSData, NSError, NSHTTPURLResponse, NSString, NSURLRequest, NSURLRequestCachePolicy,
        NSURLResponse, NSURLSession, NSURLSessionConfiguration,
    };
    use tokio::sync::oneshot;

    const URL: &str = "http://captive.apple.com/hotspot-detect.html";
    const TIMEOUT: Duration = Duration::from_secs(5);

    #[derive(Debug)]
    struct Response {
        status: Option<u16>,
        final_url: Option<String>,
        body: Vec<u8>,
        error: Option<String>,
    }

    let started = Instant::now();
    let (sender, receiver) = oneshot::channel();
    let sender = Mutex::new(Some(sender));

    let Some(url) = objc2_foundation::NSURL::URLWithString(&NSString::from_str(URL)) else {
        return failed_macos_probe(
            started,
            "internet_invalid_probe_url",
            "invalid Apple probe URL",
        );
    };
    let request = NSURLRequest::requestWithURL_cachePolicy_timeoutInterval(
        &url,
        NSURLRequestCachePolicy::ReloadIgnoringLocalAndRemoteCacheData,
        TIMEOUT.as_secs_f64(),
    );
    let configuration = NSURLSessionConfiguration::ephemeralSessionConfiguration();
    configuration
        .setRequestCachePolicy(NSURLRequestCachePolicy::ReloadIgnoringLocalAndRemoteCacheData);
    configuration.setTimeoutIntervalForRequest(TIMEOUT.as_secs_f64());
    configuration.setWaitsForConnectivity(false);
    configuration.setHTTPShouldSetCookies(false);
    configuration.setURLCache(None);
    let session = NSURLSession::sessionWithConfiguration(&configuration);

    let completion = RcBlock::new(
        move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
            let result = unsafe {
                let response = response.as_ref();
                let status = response
                    .and_then(|value| value.downcast_ref::<NSHTTPURLResponse>())
                    .map(|value| value.statusCode() as u16);
                let final_url = response
                    .and_then(NSURLResponse::URL)
                    .and_then(|value| value.absoluteString())
                    .map(|value| value.to_string());
                let body = data.as_ref().map(copy_nsdata).unwrap_or_default();
                let error = error
                    .as_ref()
                    .map(|value| value.localizedDescription().to_string());

                Response {
                    status,
                    final_url,
                    body,
                    error,
                }
            };

            if let Some(sender) = sender.lock().ok().and_then(|mut sender| sender.take()) {
                let _ = sender.send(result);
            }
        },
    );

    let task = unsafe { session.dataTaskWithRequest_completionHandler(&request, &completion) };
    task.resume();
    drop(task);
    drop(completion);

    let response = tokio::time::timeout(TIMEOUT + Duration::from_secs(1), receiver).await;
    session.invalidateAndCancel();
    let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;

    match response {
        Ok(Ok(response)) if response.error.is_none() => classify_macos_response(
            duration_ms,
            response.status,
            response.final_url,
            &response.body,
        ),
        Ok(Ok(response)) => (
            InternetProbeResult {
                status: InternetProbeStatus::Failed,
                duration_ms,
                http_status: response.status,
                error: Some(ProbeError {
                    code: "internet_probe_failed".to_string(),
                    message: response
                        .error
                        .unwrap_or_else(|| "Apple probe failed".to_string()),
                }),
            },
            false,
        ),
        Ok(Err(_)) => failed_macos_probe(
            started,
            "internet_probe_cancelled",
            "Apple probe completion channel closed",
        ),
        Err(_) => failed_macos_probe(
            started,
            "internet_timeout",
            "internet probe timed out after 5000ms",
        ),
    }

    unsafe fn copy_nsdata(data: &NSData) -> Vec<u8> {
        let mut bytes = vec![0_u8; data.length()];
        if let Some(pointer) = NonNull::new(bytes.as_mut_ptr().cast::<c_void>()) {
            data.getBytes_length(pointer, bytes.len());
        }
        bytes
    }
}

#[cfg(any(target_os = "macos", test))]
fn classify_macos_response(
    duration_ms: u64,
    http_status: Option<u16>,
    final_url: Option<String>,
    body: &[u8],
) -> (InternetProbeResult, bool) {
    const URL: &str = "http://captive.apple.com/hotspot-detect.html";

    let body = String::from_utf8_lossy(body);
    let normalized_body = body.to_ascii_lowercase();
    let body_matches =
        normalized_body.contains("<body>success</body>") || normalized_body.trim() == "success";
    let url_matches = final_url.as_deref() == Some(URL);

    if http_status == Some(200) && url_matches && body_matches {
        return (
            InternetProbeResult {
                status: InternetProbeStatus::Success,
                duration_ms,
                http_status,
                error: None,
            },
            false,
        );
    }

    let captive_portal = matches!(http_status, Some(200..=399)) && (!url_matches || !body_matches);
    (
        InternetProbeResult {
            status: InternetProbeStatus::UnexpectedResponse,
            duration_ms,
            http_status,
            error: Some(ProbeError {
                code: if captive_portal {
                    "captive_portal_detected".to_string()
                } else {
                    "internet_unexpected_response".to_string()
                },
                message: format!(
                    "expected Apple captive probe success response, received HTTP {} from {}",
                    http_status
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    final_url.unwrap_or_else(|| "unknown URL".to_string())
                ),
            }),
        },
        captive_portal,
    )
}

#[cfg(target_os = "macos")]
fn failed_macos_probe(started: Instant, code: &str, message: &str) -> (InternetProbeResult, bool) {
    (
        InternetProbeResult {
            status: InternetProbeStatus::Failed,
            duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            http_status: None,
            error: Some(ProbeError {
                code: code.to_string(),
                message: message.to_string(),
            }),
        },
        false,
    )
}

#[cfg(target_os = "windows")]
fn windows_ncsi_hint() -> InternetHintLevel {
    use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation};

    let Ok(profile) = NetworkInformation::GetInternetConnectionProfile() else {
        return InternetHintLevel::Unknown;
    };
    let Ok(level) = profile.GetNetworkConnectivityLevel() else {
        return InternetHintLevel::Unknown;
    };

    match level {
        NetworkConnectivityLevel::None => InternetHintLevel::None,
        NetworkConnectivityLevel::LocalAccess => InternetHintLevel::LocalAccess,
        NetworkConnectivityLevel::ConstrainedInternetAccess => {
            InternetHintLevel::ConstrainedInternetAccess
        }
        NetworkConnectivityLevel::InternetAccess => InternetHintLevel::InternetAccess,
        _ => InternetHintLevel::Unknown,
    }
}

#[cfg(target_os = "windows")]
async fn windows_active_probe() -> (InternetProbeResult, bool) {
    use windows::{
        core::HSTRING,
        Foundation::Uri,
        Web::Http::{
            Filters::{HttpBaseProtocolFilter, HttpCacheReadBehavior, HttpCacheWriteBehavior},
            HttpClient,
        },
    };

    const URL: &str = "http://www.msftconnecttest.com/connecttest.txt";
    const EXPECTED_BODY: &str = "Microsoft Connect Test";
    const TIMEOUT: Duration = Duration::from_secs(5);

    let started = Instant::now();
    let result = tokio::time::timeout(TIMEOUT, async {
        let response_operation = {
            let filter = HttpBaseProtocolFilter::new()?;
            filter.SetAllowAutoRedirect(false)?;
            let cache = filter.CacheControl()?;
            cache.SetReadBehavior(HttpCacheReadBehavior::NoCache)?;
            cache.SetWriteBehavior(HttpCacheWriteBehavior::NoCache)?;
            let client = HttpClient::Create(&filter)?;
            let uri = Uri::CreateUri(&HSTRING::from(URL))?;
            client.GetAsync(&uri)?
        };
        let response = response_operation.await?;
        let status = response.StatusCode()?.0 as u16;
        let body_operation = response.Content()?.ReadAsStringAsync()?;
        drop(response);
        let body = body_operation.await?.to_string();
        windows::core::Result::Ok((status, body))
    })
    .await;
    let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;

    match result {
        Ok(Ok((200, body))) if body.trim() == EXPECTED_BODY => (
            InternetProbeResult {
                status: InternetProbeStatus::Success,
                duration_ms,
                http_status: Some(200),
                error: None,
            },
            false,
        ),
        Ok(Ok((status, body))) => {
            let captive_portal =
                (300..400).contains(&status) || (status == 200 && body.trim() != EXPECTED_BODY);
            (
                InternetProbeResult {
                    status: InternetProbeStatus::UnexpectedResponse,
                    duration_ms,
                    http_status: Some(status),
                    error: Some(ProbeError {
                        code: if captive_portal {
                            "captive_portal_detected".to_string()
                        } else {
                            "internet_unexpected_response".to_string()
                        },
                        message: format!(
                            "expected HTTP 200 with Microsoft Connect Test, received HTTP {status}"
                        ),
                    }),
                },
                captive_portal,
            )
        }
        Ok(Err(error)) => (
            InternetProbeResult {
                status: InternetProbeStatus::Failed,
                duration_ms,
                http_status: None,
                error: Some(ProbeError {
                    code: "internet_probe_failed".to_string(),
                    message: error.to_string(),
                }),
            },
            false,
        ),
        Err(_) => (
            InternetProbeResult {
                status: InternetProbeStatus::Failed,
                duration_ms,
                http_status: None,
                error: Some(ProbeError {
                    code: "internet_timeout".to_string(),
                    message: "internet probe timed out after 5000ms".to_string(),
                }),
            },
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(status: InternetProbeStatus, http_status: Option<u16>) -> InternetProbeResult {
        InternetProbeResult {
            status,
            duration_ms: 42,
            http_status,
            error: None,
        }
    }

    #[test]
    fn exact_active_probe_success_is_verified_internet() {
        let snapshot = classify_platform_evidence(
            InternetHintSource::WindowsNcsi,
            InternetHintLevel::InternetAccess,
            probe(InternetProbeStatus::Success, Some(200)),
            false,
        );

        assert_eq!(snapshot.status, InternetStatus::Available);
        assert!(snapshot.verified);
        assert_eq!(snapshot.reason, "internet_probe_succeeded");
    }

    #[test]
    fn unexpected_content_is_captive_portal() {
        let snapshot = classify_platform_evidence(
            InternetHintSource::WindowsNcsi,
            InternetHintLevel::ConstrainedInternetAccess,
            probe(InternetProbeStatus::UnexpectedResponse, Some(200)),
            true,
        );

        assert_eq!(snapshot.status, InternetStatus::CaptivePortal);
        assert!(!snapshot.verified);
        assert!(snapshot.captive_portal);
    }

    #[test]
    fn ncsi_internet_with_failed_active_probe_is_degraded() {
        let snapshot = classify_platform_evidence(
            InternetHintSource::WindowsNcsi,
            InternetHintLevel::InternetAccess,
            probe(InternetProbeStatus::Failed, None),
            false,
        );

        assert_eq!(snapshot.status, InternetStatus::Degraded);
        assert!(!snapshot.verified);
    }

    #[test]
    fn local_access_with_failed_active_probe_is_unavailable() {
        let snapshot = classify_platform_evidence(
            InternetHintSource::WindowsNcsi,
            InternetHintLevel::LocalAccess,
            probe(InternetProbeStatus::Failed, None),
            false,
        );

        assert_eq!(snapshot.status, InternetStatus::Unavailable);
        assert!(!snapshot.verified);
    }

    #[test]
    fn repeated_active_probe_failures_override_stale_ncsi_hint() {
        let failed_probe = || {
            classify_platform_evidence(
                InternetHintSource::WindowsNcsi,
                InternetHintLevel::InternetAccess,
                probe(InternetProbeStatus::Failed, None),
                false,
            )
        };
        let first = apply_failure_policy(&InternetSnapshot::default(), failed_probe());
        let second = apply_failure_policy(&first, failed_probe());
        let third = apply_failure_policy(&second, failed_probe());

        assert_eq!(first.status, InternetStatus::Degraded);
        assert_eq!(second.status, InternetStatus::Degraded);
        assert_eq!(third.status, InternetStatus::Unavailable);
        assert_eq!(third.consecutive_failures, INTERNET_FAILURE_THRESHOLD);
        assert_eq!(third.reason, "internet_probe_failure_threshold_reached");
    }

    #[test]
    fn unavailable_internet_uses_fast_recovery_retry() {
        let monitor = InternetMonitor {
            last_checked: Some(Instant::now() - INTERNET_RECOVERY_INTERVAL),
            snapshot: unavailable_without_interface(),
        };

        assert_eq!(monitor.next_check_in(Some(true)), Duration::ZERO);
        assert!(monitor.is_due());
    }

    #[test]
    fn persistent_unavailable_internet_uses_bounded_backoff() {
        let now = Instant::now();
        let mut snapshot = unavailable_without_interface();
        snapshot.consecutive_failures = INTERNET_FAILURE_THRESHOLD + 2;
        let monitor = InternetMonitor {
            last_checked: Some(now),
            snapshot,
        };

        assert_eq!(monitor.check_interval(), Duration::from_secs(40));
    }

    #[test]
    fn network_change_reset_is_immediately_due() {
        let mut monitor = InternetMonitor {
            last_checked: Some(Instant::now()),
            snapshot: classify_platform_evidence(
                InternetHintSource::WindowsNcsi,
                InternetHintLevel::InternetAccess,
                probe(InternetProbeStatus::Success, Some(200)),
                false,
            ),
        };

        monitor.reset_for_network_change();

        assert_eq!(monitor.next_check_in(Some(true)), Duration::ZERO);
        assert_eq!(monitor.snapshot.status, InternetStatus::Unknown);
    }

    #[test]
    fn unsupported_platform_does_not_converge_unknown_to_unavailable() {
        let mut previous = InternetSnapshot::default();

        for _ in 0..10 {
            previous =
                apply_failure_policy(&previous, unknown_snapshot("internet_check_unsupported"));
        }

        assert_eq!(previous.status, InternetStatus::Unknown);
        assert_eq!(previous.consecutive_failures, 0);
        assert_eq!(previous.reason, "internet_check_unsupported");
    }

    #[test]
    fn macos_success_page_is_verified() {
        let (probe, captive_portal) = classify_macos_response(
            31,
            Some(200),
            Some("http://captive.apple.com/hotspot-detect.html".to_string()),
            b"<HTML><HEAD><TITLE>Success</TITLE></HEAD><BODY>Success</BODY></HTML>",
        );

        assert_eq!(probe.status, InternetProbeStatus::Success);
        assert!(!captive_portal);
    }

    #[test]
    fn macos_redirected_success_response_is_captive_portal() {
        let (probe, captive_portal) = classify_macos_response(
            45,
            Some(200),
            Some("http://login.example.test/portal".to_string()),
            b"<html><body>Sign in</body></html>",
        );

        assert_eq!(probe.status, InternetProbeStatus::UnexpectedResponse);
        assert!(captive_portal);
        assert_eq!(
            probe.error.map(|error| error.code).as_deref(),
            Some("captive_portal_detected")
        );
    }

    #[test]
    fn platform_classifier_preserves_macos_hint_source() {
        let snapshot = classify_platform_evidence(
            InternetHintSource::MacosReachability,
            InternetHintLevel::InternetAccess,
            probe(InternetProbeStatus::Success, Some(200)),
            false,
        );

        assert_eq!(
            snapshot.system_hint.source,
            InternetHintSource::MacosReachability
        );
        assert_eq!(snapshot.status, InternetStatus::Available);
    }
}
