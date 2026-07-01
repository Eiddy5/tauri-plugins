# Net Watcher Plugin Design

Date: 2026-07-02

## Goal

Build a Tauri v2 desktop plugin that monitors device network availability and network quality on Windows and macOS.

The plugin should answer two questions:

1. What network is the device currently using?
2. Is the configured target reachable with acceptable quality?

The first release targets Windows and macOS only. Mobile platforms are out of scope and should return an unsupported error if called.

## Non-Goals

- No LAN device discovery.
- No packet capture.
- No custom long-link or short-link protocol stack.
- No ICMP-only dependency.
- No Android or iOS implementation in the first release.

## Configuration

The plugin reads default configuration from `tauri.conf.json` under `plugins.net-watcher`.

```json
{
  "plugins": {
    "net-watcher": {
      "autoStart": true,
      "target": "https://example.com/health",
      "intervalMs": 10000,
      "timeoutMs": 3000
    }
  }
}
```

Only core behavior is configurable in the first release:

- `autoStart`: starts monitoring during plugin setup when true.
- `target`: HTTP or HTTPS URL used for active quality probing.
- `intervalMs`: delay between quality probes.
- `timeoutMs`: timeout for a single probe.

Internal defaults:

- `autoStart`: `false`
- `target`: `https://www.apple.com/library/test/success.html`
- `intervalMs`: `10000`
- `timeoutMs`: `3000`
- `windowSize`: `20`
- `degradedFailureRate`: `0.15`
- `degradedP95LatencyMs`: `800`
- `offlineConsecutiveFailures`: `3`
- `includeMacAddress`: `false`

Runtime options passed to `startWatching` may override `target`, `intervalMs`, and `timeoutMs` for that watcher session.

## Public API

Rust commands exposed through the Tauri plugin:

- `getSnapshot()`: returns the latest `NetWatcherSnapshot`.
- `startWatching(options?)`: starts network monitoring and quality probing.
- `stopWatching()`: stops monitoring background tasks.
- `getConfig()`: returns the effective configuration.

Guest JavaScript API:

```ts
export function getSnapshot(): Promise<NetWatcherSnapshot>
export function startWatching(options?: StartWatchingOptions): Promise<void>
export function stopWatching(): Promise<void>
export function getConfig(): Promise<NetWatcherConfig>
export function onSnapshotUpdated(handler: (snapshot: NetWatcherSnapshot) => void): Promise<UnlistenFn>
```

The plugin emits one primary event:

```text
net-watcher://snapshot-updated
```

The event payload is always a complete `NetWatcherSnapshot`. Consumers can read `snapshot.state.overall` for simple UI logic and inspect `network` or `quality` for diagnostics.

## Snapshot Structure

```json
{
  "meta": {
    "snapshotId": "nw_20260702_012435_001",
    "timestamp": "2026-07-02T01:24:35.218+08:00",
    "platform": "windows",
    "pluginVersion": "0.1.0"
  },
  "state": {
    "overall": "degraded",
    "network": "connected",
    "quality": "unstable",
    "score": 68,
    "reason": "high_latency_and_recent_failures"
  },
  "network": {
    "primaryInterfaceId": "if_wifi_0",
    "interfaces": [
      {
        "id": "if_wifi_0",
        "name": "Wi-Fi",
        "displayName": "Intel(R) Wi-Fi 6 AX201",
        "type": "wifi",
        "status": "up",
        "isPrimary": true,
        "addresses": {
          "ipv4": ["192.168.1.23"],
          "ipv6": ["fe80::a12b:34ff:fe56:7890"],
          "mac": null
        },
        "gateway": "192.168.1.1",
        "dnsServers": ["192.168.1.1", "8.8.8.8"]
      }
    ]
  },
  "quality": {
    "config": {
      "intervalMs": 10000,
      "windowSize": 20,
      "timeoutMs": 3000
    },
    "target": {
      "type": "http",
      "url": "https://example.com/health"
    },
    "currentProbe": {
      "id": "probe_00192",
      "status": "success",
      "startedAt": "2026-07-02T01:24:34.982+08:00",
      "endedAt": "2026-07-02T01:24:35.218+08:00",
      "durationMs": 236,
      "phases": {
        "dnsMs": 28,
        "tcpMs": 74,
        "tlsMs": 91,
        "httpMs": 43
      },
      "http": {
        "statusCode": 204
      },
      "error": null
    },
    "summary": {
      "sampleCount": 20,
      "successCount": 17,
      "failureCount": 3,
      "failureRate": 0.15,
      "latencyMs": {
        "avg": 184,
        "min": 72,
        "max": 822,
        "p95": 610
      },
      "jitterMs": 96,
      "consecutiveFailures": 0,
      "lastSuccessAt": "2026-07-02T01:24:35.218+08:00",
      "lastFailureAt": "2026-07-02T01:23:45.114+08:00",
      "lastFailureReason": "tcp_timeout"
    }
  },
  "changes": {
    "hasChanges": true,
    "previousOverall": "online",
    "currentOverall": "degraded",
    "changedFields": [
      "state.overall",
      "state.quality",
      "quality.summary.failureRate",
      "quality.summary.latencyMs.p95"
    ]
  }
}
```

## Monitoring Logic

The plugin uses two cooperating background components.

`Network Watcher` observes system network state:

- Available network interfaces.
- Primary interface.
- Interface type, such as Wi-Fi, Ethernet, VPN, loopback, or unknown.
- Interface up/down state.
- IPv4 and IPv6 addresses.
- Default gateway.
- DNS servers.

`Quality Monitor` actively probes the configured target:

- Resolve DNS.
- Open TCP connection.
- Perform TLS handshake for HTTPS.
- Send HTTP request and measure response timing.
- Record success, failure reason, status code, and duration.

Each probe result is inserted into a rolling window. The rolling window computes failure rate, latency statistics, jitter, consecutive failures, and last success or failure time.

## State Machine

The final state is derived from system network data and rolling quality statistics.

- `unknown`: plugin just started or has insufficient data.
- `offline`: no usable network interface is available.
- `localOnly`: a network interface is available, but the configured target is repeatedly unreachable.
- `degraded`: the target is reachable, but latency, jitter, or recent failure rate is poor.
- `online`: the target is reachable and recent quality is stable.

State details:

- `state.overall`: one of `unknown`, `offline`, `localOnly`, `degraded`, or `online`.
- `state.network`: system-level state, such as `unknown`, `disconnected`, or `connected`.
- `state.quality`: quality-level state, such as `unknown`, `unreachable`, `unstable`, or `stable`.
- `state.score`: 0 to 100 quality score.
- `state.reason`: machine-readable reason for the current state.

Initial scoring rules:

- Start from 100.
- Penalize recent failure rate.
- Penalize high P95 latency.
- Penalize high jitter.
- Penalize consecutive failures.
- Clamp the final score to 0 through 100.

## Platform Strategy

Windows and macOS share the same public API, data model, state machine, rolling window, and probe logic.

Platform-specific code is limited to system network observation:

- Windows implementation reads and watches network adapter, IP, gateway, and DNS changes through native system APIs.
- macOS implementation reads and watches interface, route, and DNS changes through native system APIs.

If native event subscriptions are incomplete or unreliable, the plugin may use a conservative polling fallback for system network state. Quality probing remains cross-platform.

## Error Handling

Errors should be structured and serializable:

- `unsupported_platform`
- `invalid_config`
- `already_watching`
- `not_watching`
- `probe_failed`
- `system_network_unavailable`
- `internal_error`

Probe failures are not always command failures. A failed probe should normally appear in `quality.currentProbe.error` and update the snapshot, not reject the whole watcher task.

## Privacy

MAC address is not returned by default.

The first release should not expose packet contents, nearby devices, SSIDs, or LAN discovery data. The plugin should only expose network interface metadata and active probe results required for application health diagnostics.

## Testing Strategy

Unit tests:

- Configuration defaulting and runtime override merge.
- Rolling window statistics.
- State machine transitions.
- Score calculation.
- Error serialization.

Integration tests where practical:

- `getSnapshot` returns a valid default snapshot before watching.
- `startWatching` creates probe results.
- `stopWatching` stops background work.
- Event payload is a complete snapshot.

Manual platform verification:

- Windows Wi-Fi disconnect and reconnect.
- Windows switching between Wi-Fi and Ethernet.
- macOS Wi-Fi disconnect and reconnect.
- macOS switching DNS or VPN.
- Target timeout and recovery.

