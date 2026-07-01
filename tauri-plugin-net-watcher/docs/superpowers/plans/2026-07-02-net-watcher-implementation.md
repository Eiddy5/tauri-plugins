# Net Watcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 Windows 和 macOS 可用的 Tauri v2 网络状态与网络质量监控插件。

**Architecture:** Rust 侧按配置、模型、滚动统计、状态机、主动探测、系统网络观察、Tauri 命令分层。第一版用统一快照事件向前端推送完整 `NetWatcherSnapshot`，平台差异限制在 `network` 适配层内。

**Tech Stack:** Rust 2021、Tauri v2、Serde、Tokio、TypeScript、Rollup、Tauri permissions。

---

## 文件结构

- `Cargo.toml`：补充异步、时间、URL、TLS、HTTP 探测和网络接口读取依赖。
- `build.rs`：把 `ping` 替换成真实命令列表。
- `permissions/default.toml`：把默认权限切换到真实命令。
- `src/config.rs`：插件配置、默认值、运行时覆盖合并。
- `src/models.rs`：快照、网络、质量、事件 payload 的序列化模型。
- `src/stats.rs`：滚动窗口、延迟统计、失败率、抖动、P95。
- `src/state.rs`：状态机和评分规则。
- `src/probe.rs`：HTTP/HTTPS 主动探测。
- `src/network.rs`：跨平台网络接口快照读取。第一版使用轮询读取，后续可替换为原生事件订阅。
- `src/desktop.rs`：桌面端 watcher 状态、后台任务、事件推送。
- `src/commands.rs`：Tauri 命令入口。
- `src/error.rs`：结构化错误码。
- `src/lib.rs`：注册命令、读取插件配置、启动 autoStart。
- `src/mobile.rs`：移动端返回 `unsupported_platform`。
- `guest-js/index.ts`：前端 API 和类型导出。
- `examples/tauri-app/src/App.svelte`：示例页面调用真实 API。
- `examples/tauri-app/src-tauri/tauri.conf.json`：示例插件配置。
- `README.md`：中文使用说明。

## 执行前检查

- [ ] **Step 1: 查看当前工作区状态**

Run:

```powershell
git status --short
```

Expected: 允许看到插件模板文件仍为未跟踪状态，但不要回滚用户已有改动。

- [ ] **Step 2: 确认当前插件能编译到模板状态**

Run:

```powershell
cargo check
```

Expected: 当前模板能通过，或只暴露现有模板缺少依赖锁文件相关问题。记录失败输出后继续从 Task 1 开始。

---

### Task 1: 配置模型

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs`
- Test: `src/config.rs`

- [ ] **Step 1: 写配置测试**

在 `src/config.rs` 新增测试模块：

```rust
#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_config_uses_core_defaults() {
    let config = NetWatcherConfig::default();

    assert!(!config.auto_start);
    assert_eq!(config.target, "https://www.apple.com/library/test/success.html");
    assert_eq!(config.interval_ms, 10_000);
    assert_eq!(config.timeout_ms, 3_000);
    assert_eq!(config.window_size, 20);
    assert_eq!(config.degraded_failure_rate, 0.15);
    assert_eq!(config.degraded_p95_latency_ms, 800);
    assert_eq!(config.offline_consecutive_failures, 3);
    assert!(!config.include_mac_address);
  }

  #[test]
  fn runtime_options_override_session_values_only() {
    let base = NetWatcherConfig::default();
    let options = StartWatchingOptions {
      target: Some("https://api.example.com/health".to_string()),
      interval_ms: Some(5_000),
      timeout_ms: Some(1_500),
    };

    let merged = base.with_runtime_options(options);

    assert_eq!(merged.target, "https://api.example.com/health");
    assert_eq!(merged.interval_ms, 5_000);
    assert_eq!(merged.timeout_ms, 1_500);
    assert_eq!(merged.window_size, 20);
  }

  #[test]
  fn invalid_runtime_values_are_rejected() {
    let options = StartWatchingOptions {
      target: Some("file:///tmp/health".to_string()),
      interval_ms: Some(0),
      timeout_ms: Some(0),
    };

    let error = NetWatcherConfig::default()
      .with_runtime_options(options)
      .validate()
      .unwrap_err();

    assert_eq!(error.code(), "invalid_config");
  }
}
```

- [ ] **Step 2: 运行测试并确认失败**

Run:

```powershell
cargo test config
```

Expected: FAIL，错误包含 `NetWatcherConfig` 或 `StartWatchingOptions` 未定义。

- [ ] **Step 3: 实现配置模型**

创建 `src/config.rs`：

```rust
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const DEFAULT_TARGET: &str = "https://www.apple.com/library/test/success.html";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetWatcherConfig {
  #[serde(default)]
  pub auto_start: bool,
  #[serde(default = "default_target")]
  pub target: String,
  #[serde(default = "default_interval_ms")]
  pub interval_ms: u64,
  #[serde(default = "default_timeout_ms")]
  pub timeout_ms: u64,
  #[serde(default = "default_window_size")]
  pub window_size: usize,
  #[serde(default = "default_degraded_failure_rate")]
  pub degraded_failure_rate: f64,
  #[serde(default = "default_degraded_p95_latency_ms")]
  pub degraded_p95_latency_ms: u64,
  #[serde(default = "default_offline_consecutive_failures")]
  pub offline_consecutive_failures: usize,
  #[serde(default)]
  pub include_mac_address: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartWatchingOptions {
  pub target: Option<String>,
  pub interval_ms: Option<u64>,
  pub timeout_ms: Option<u64>,
}

impl Default for NetWatcherConfig {
  fn default() -> Self {
    Self {
      auto_start: false,
      target: default_target(),
      interval_ms: default_interval_ms(),
      timeout_ms: default_timeout_ms(),
      window_size: default_window_size(),
      degraded_failure_rate: default_degraded_failure_rate(),
      degraded_p95_latency_ms: default_degraded_p95_latency_ms(),
      offline_consecutive_failures: default_offline_consecutive_failures(),
      include_mac_address: false,
    }
  }
}

impl NetWatcherConfig {
  pub fn with_runtime_options(mut self, options: StartWatchingOptions) -> Self {
    if let Some(target) = options.target {
      self.target = target;
    }
    if let Some(interval_ms) = options.interval_ms {
      self.interval_ms = interval_ms;
    }
    if let Some(timeout_ms) = options.timeout_ms {
      self.timeout_ms = timeout_ms;
    }
    self
  }

  pub fn validate(&self) -> Result<()> {
    let is_http = self.target.starts_with("http://") || self.target.starts_with("https://");
    if !is_http || self.interval_ms == 0 || self.timeout_ms == 0 || self.window_size == 0 {
      return Err(Error::invalid_config("target must be http(s), intervalMs, timeoutMs, and windowSize must be positive"));
    }
    Ok(())
  }
}

fn default_target() -> String {
  DEFAULT_TARGET.to_string()
}

fn default_interval_ms() -> u64 {
  10_000
}

fn default_timeout_ms() -> u64 {
  3_000
}

fn default_window_size() -> usize {
  20
}

fn default_degraded_failure_rate() -> f64 {
  0.15
}

fn default_degraded_p95_latency_ms() -> u64 {
  800
}

fn default_offline_consecutive_failures() -> usize {
  3
}
```

在 `src/lib.rs` 添加：

```rust
mod config;

pub use config::{NetWatcherConfig, StartWatchingOptions};
```

- [ ] **Step 4: 运行测试并确认通过**

Run:

```powershell
cargo test config
```

Expected: PASS。

- [ ] **Step 5: 提交配置模型**

Run:

```powershell
git add tauri-plugin-net-watcher/src/config.rs tauri-plugin-net-watcher/src/lib.rs
git commit -m "feat: add net watcher config model"
```

---

### Task 2: 错误类型和快照数据模型

**Files:**
- Modify: `src/error.rs`
- Replace: `src/models.rs`
- Test: `src/models.rs`

- [ ] **Step 1: 写模型序列化测试**

在 `src/models.rs` 添加测试：

```rust
#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_snapshot_serializes_with_expected_state() {
    let snapshot = NetWatcherSnapshot::initial("0.1.0");
    let value = serde_json::to_value(snapshot).unwrap();

    assert_eq!(value["state"]["overall"], "unknown");
    assert_eq!(value["state"]["network"], "unknown");
    assert_eq!(value["state"]["quality"], "unknown");
    assert_eq!(value["quality"]["summary"]["sampleCount"], 0);
  }
}
```

- [ ] **Step 2: 运行测试并确认失败**

Run:

```powershell
cargo test models
```

Expected: FAIL，错误包含 `NetWatcherSnapshot` 未定义。

- [ ] **Step 3: 更新依赖**

修改 `Cargo.toml`：

```toml
[dependencies]
tauri = { version = "2.10.0" }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "2"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
```

- [ ] **Step 4: 实现结构化错误**

替换 `src/error.rs`：

```rust
use serde::{ser::Serializer, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
  #[error("{code}: {message}")]
  Structured { code: &'static str, message: String },
  #[error(transparent)]
  Io(#[from] std::io::Error),
  #[cfg(mobile)]
  #[error(transparent)]
  PluginInvoke(#[from] tauri::plugin::mobile::PluginInvokeError),
}

impl Error {
  pub fn unsupported_platform() -> Self {
    Self::Structured {
      code: "unsupported_platform",
      message: "net-watcher does not support this platform".to_string(),
    }
  }

  pub fn invalid_config(message: impl Into<String>) -> Self {
    Self::Structured {
      code: "invalid_config",
      message: message.into(),
    }
  }

  pub fn already_watching() -> Self {
    Self::Structured {
      code: "already_watching",
      message: "net-watcher is already running".to_string(),
    }
  }

  pub fn not_watching() -> Self {
    Self::Structured {
      code: "not_watching",
      message: "net-watcher is not running".to_string(),
    }
  }

  pub fn internal(message: impl Into<String>) -> Self {
    Self::Structured {
      code: "internal_error",
      message: message.into(),
    }
  }

  pub fn code(&self) -> &'static str {
    match self {
      Self::Structured { code, .. } => code,
      Self::Io(_) => "system_network_unavailable",
      #[cfg(mobile)]
      Self::PluginInvoke(_) => "internal_error",
    }
  }
}

impl Serialize for Error {
  fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
  where
    S: Serializer,
  {
    serializer.serialize_str(self.to_string().as_ref())
  }
}
```

- [ ] **Step 5: 实现快照模型**

替换 `src/models.rs` 为快照模型，保留 camelCase 序列化：

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OverallState {
  Unknown,
  Offline,
  LocalOnly,
  Degraded,
  Online,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NetworkLayerState {
  Unknown,
  Disconnected,
  Connected,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum QualityLayerState {
  Unknown,
  Unreachable,
  Unstable,
  Stable,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetWatcherSnapshot {
  pub meta: SnapshotMeta,
  pub state: SnapshotState,
  pub network: NetworkSnapshot,
  pub quality: QualitySnapshot,
  pub changes: SnapshotChanges,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMeta {
  pub snapshot_id: String,
  pub timestamp: DateTime<Utc>,
  pub platform: String,
  pub plugin_version: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotState {
  pub overall: OverallState,
  pub network: NetworkLayerState,
  pub quality: QualityLayerState,
  pub score: u8,
  pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSnapshot {
  pub primary_interface_id: Option<String>,
  pub interfaces: Vec<NetworkInterface>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInterface {
  pub id: String,
  pub name: String,
  pub display_name: String,
  #[serde(rename = "type")]
  pub interface_type: InterfaceType,
  pub status: InterfaceStatus,
  pub is_primary: bool,
  pub addresses: InterfaceAddresses,
  pub gateway: Option<String>,
  pub dns_servers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InterfaceType {
  Wifi,
  Ethernet,
  Vpn,
  Loopback,
  Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InterfaceStatus {
  Up,
  Down,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceAddresses {
  pub ipv4: Vec<String>,
  pub ipv6: Vec<String>,
  pub mac: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualitySnapshot {
  pub config: QualityConfigSnapshot,
  pub target: ProbeTarget,
  pub current_probe: Option<ProbeResult>,
  pub summary: QualitySummary,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityConfigSnapshot {
  pub interval_ms: u64,
  pub window_size: usize,
  pub timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeTarget {
  #[serde(rename = "type")]
  pub target_type: ProbeTargetType,
  pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProbeTargetType {
  Http,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
  pub id: String,
  pub status: ProbeStatus,
  pub started_at: DateTime<Utc>,
  pub ended_at: DateTime<Utc>,
  pub duration_ms: u64,
  pub phases: ProbePhases,
  pub http: Option<HttpProbeResult>,
  pub error: Option<ProbeError>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProbeStatus {
  Success,
  Failed,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbePhases {
  pub dns_ms: Option<u64>,
  pub tcp_ms: Option<u64>,
  pub tls_ms: Option<u64>,
  pub http_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpProbeResult {
  pub status_code: u16,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeError {
  pub code: String,
  pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualitySummary {
  pub sample_count: usize,
  pub success_count: usize,
  pub failure_count: usize,
  pub failure_rate: f64,
  pub latency_ms: LatencySummary,
  pub jitter_ms: u64,
  pub consecutive_failures: usize,
  pub last_success_at: Option<DateTime<Utc>>,
  pub last_failure_at: Option<DateTime<Utc>>,
  pub last_failure_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencySummary {
  pub avg: u64,
  pub min: u64,
  pub max: u64,
  pub p95: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotChanges {
  pub has_changes: bool,
  pub previous_overall: Option<OverallState>,
  pub current_overall: OverallState,
  pub changed_fields: Vec<String>,
}

impl NetWatcherSnapshot {
  pub fn initial(plugin_version: &str) -> Self {
    let overall = OverallState::Unknown;
    Self {
      meta: SnapshotMeta {
        snapshot_id: format!("nw_{}", Uuid::new_v4()),
        timestamp: Utc::now(),
        platform: std::env::consts::OS.to_string(),
        plugin_version: plugin_version.to_string(),
      },
      state: SnapshotState {
        overall: overall.clone(),
        network: NetworkLayerState::Unknown,
        quality: QualityLayerState::Unknown,
        score: 0,
        reason: "insufficient_data".to_string(),
      },
      network: NetworkSnapshot::default(),
      quality: QualitySnapshot {
        config: QualityConfigSnapshot {
          interval_ms: 10_000,
          window_size: 20,
          timeout_ms: 3_000,
        },
        target: ProbeTarget {
          target_type: ProbeTargetType::Http,
          url: "https://www.apple.com/library/test/success.html".to_string(),
        },
        current_probe: None,
        summary: QualitySummary::default(),
      },
      changes: SnapshotChanges {
        has_changes: false,
        previous_overall: None,
        current_overall: overall,
        changed_fields: Vec::new(),
      },
    }
  }
}
```

- [ ] **Step 6: 运行测试并确认通过**

Run:

```powershell
cargo test models
```

Expected: PASS。

- [ ] **Step 7: 提交模型**

Run:

```powershell
git add tauri-plugin-net-watcher/Cargo.toml tauri-plugin-net-watcher/src/error.rs tauri-plugin-net-watcher/src/models.rs
git commit -m "feat: add net watcher snapshot models"
```

---

### Task 3: 滚动窗口统计

**Files:**
- Create: `src/stats.rs`
- Modify: `src/lib.rs`
- Test: `src/stats.rs`

- [ ] **Step 1: 写统计测试**

在 `src/stats.rs` 添加：

```rust
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
}
```

- [ ] **Step 2: 运行测试并确认失败**

Run:

```powershell
cargo test stats
```

Expected: FAIL，错误包含 `RollingWindow` 未定义。

- [ ] **Step 3: 实现滚动窗口**

创建 `src/stats.rs`：

```rust
use std::collections::VecDeque;

use crate::models::*;

#[derive(Debug, Clone)]
pub struct RollingWindow {
  capacity: usize,
  samples: VecDeque<ProbeResult>,
}

impl RollingWindow {
  pub fn new(capacity: usize) -> Self {
    Self {
      capacity: capacity.max(1),
      samples: VecDeque::new(),
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
    let success_samples: Vec<&ProbeResult> = self
      .samples
      .iter()
      .filter(|sample| sample.status == ProbeStatus::Success)
      .collect();
    let success_count = success_samples.len();
    let failure_count = sample_count.saturating_sub(success_count);
    let failure_rate = if sample_count == 0 {
      0.0
    } else {
      failure_count as f64 / sample_count as f64
    };
    let latencies: Vec<u64> = success_samples.iter().map(|sample| sample.duration_ms).collect();
    let latency_ms = latency_summary(&latencies);
    let jitter_ms = jitter(&latencies);
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

    QualitySummary {
      sample_count,
      success_count,
      failure_count,
      failure_rate,
      latency_ms,
      jitter_ms,
      consecutive_failures,
      last_success_at,
      last_failure_at: last_failure.map(|sample| sample.ended_at),
      last_failure_reason: last_failure
        .and_then(|sample| sample.error.as_ref())
        .map(|error| error.code.clone()),
    }
  }
}

fn latency_summary(latencies: &[u64]) -> LatencySummary {
  if latencies.is_empty() {
    return LatencySummary::default();
  }

  let mut sorted = latencies.to_vec();
  sorted.sort_unstable();
  let sum: u64 = sorted.iter().sum();
  let p95_index = ((sorted.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);

  LatencySummary {
    avg: sum / sorted.len() as u64,
    min: *sorted.first().unwrap_or(&0),
    max: *sorted.last().unwrap_or(&0),
    p95: sorted[p95_index.min(sorted.len() - 1)],
  }
}

fn jitter(latencies: &[u64]) -> u64 {
  if latencies.len() < 2 {
    return 0;
  }

  let total_delta: u64 = latencies
    .windows(2)
    .map(|pair| pair[0].abs_diff(pair[1]))
    .sum();
  total_delta / (latencies.len() as u64 - 1)
}
```

在 `src/lib.rs` 添加：

```rust
mod stats;
```

- [ ] **Step 4: 运行测试并确认通过**

Run:

```powershell
cargo test stats
```

Expected: PASS。

- [ ] **Step 5: 提交统计模块**

Run:

```powershell
git add tauri-plugin-net-watcher/src/stats.rs tauri-plugin-net-watcher/src/lib.rs
git commit -m "feat: add quality rolling window"
```

---

### Task 4: 状态机和评分

**Files:**
- Create: `src/state.rs`
- Modify: `src/lib.rs`
- Test: `src/state.rs`

- [ ] **Step 1: 写状态机测试**

在 `src/state.rs` 添加：

```rust
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
}
```

- [ ] **Step 2: 运行测试并确认失败**

Run:

```powershell
cargo test state
```

Expected: FAIL，错误包含 `evaluate_state` 未定义。

- [ ] **Step 3: 实现状态机**

创建 `src/state.rs`：

```rust
use crate::models::*;

#[derive(Debug, Clone)]
pub struct StateConfig {
  pub degraded_failure_rate: f64,
  pub degraded_p95_latency_ms: u64,
  pub offline_consecutive_failures: usize,
}

pub fn evaluate_state(
  has_available_interface: bool,
  summary: &QualitySummary,
  config: &StateConfig,
) -> SnapshotState {
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

  let score = score(summary, config);
  let degraded = summary.failure_rate >= config.degraded_failure_rate
    || summary.latency_ms.p95 >= config.degraded_p95_latency_ms
    || summary.jitter_ms >= 300;

  if degraded {
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

fn score(summary: &QualitySummary, config: &StateConfig) -> u8 {
  let failure_penalty = (summary.failure_rate * 100.0).round() as i32;
  let latency_penalty = if config.degraded_p95_latency_ms == 0 {
    0
  } else {
    ((summary.latency_ms.p95 as f64 / config.degraded_p95_latency_ms as f64) * 20.0).round() as i32
  };
  let jitter_penalty = ((summary.jitter_ms as f64 / 300.0) * 20.0).round() as i32;
  let consecutive_penalty = (summary.consecutive_failures as i32) * 15;
  let raw = 100 - failure_penalty - latency_penalty - jitter_penalty - consecutive_penalty;

  raw.clamp(0, 100) as u8
}
```

在 `src/lib.rs` 添加：

```rust
mod state;
```

- [ ] **Step 4: 运行测试并确认通过**

Run:

```powershell
cargo test state
```

Expected: PASS。

- [ ] **Step 5: 提交状态机**

Run:

```powershell
git add tauri-plugin-net-watcher/src/state.rs tauri-plugin-net-watcher/src/lib.rs
git commit -m "feat: add net watcher state machine"
```

---

### Task 5: HTTP/HTTPS 主动探测

**Files:**
- Create: `src/probe.rs`
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Test: `src/probe.rs`

- [ ] **Step 1: 写 URL 校验和失败探测测试**

在 `src/probe.rs` 添加：

```rust
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
```

- [ ] **Step 2: 更新依赖并运行失败测试**

修改 `Cargo.toml`：

```toml
tokio = { version = "1", features = ["io-util", "net", "sync", "time"] }
url = "2"
native-tls = "0.2"
tokio-native-tls = "0.3"
```

Run:

```powershell
cargo test probe
```

Expected: FAIL，错误包含 `HttpProber` 未定义。

- [ ] **Step 3: 实现探测器**

创建 `src/probe.rs`：

```rust
use std::{net::SocketAddr, time::Duration};

use chrono::Utc;
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::{lookup_host, TcpStream},
  time::{timeout, Instant},
};
use tokio_native_tls::TlsConnector;
use url::Url;
use uuid::Uuid;

use crate::models::*;

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
    let outcome = timeout(Duration::from_millis(self.timeout_ms), self.probe_inner(target, &mut phases)).await;
    let ended_at = Utc::now();
    let duration_ms = started.elapsed().as_millis() as u64;

    match outcome {
      Ok(Ok(status_code)) => ProbeResult {
        id: format!("probe_{}", Uuid::new_v4()),
        status: ProbeStatus::Success,
        started_at,
        ended_at,
        duration_ms,
        phases,
        http: Some(HttpProbeResult { status_code }),
        error: None,
      },
      Ok(Err(error)) => failed_probe(started_at, ended_at, duration_ms, phases, error.0, error.1),
      Err(_) => failed_probe(
        started_at,
        ended_at,
        duration_ms,
        phases,
        "http_timeout",
        "probe timed out",
      ),
    }
  }

  async fn probe_inner(
    &self,
    target: &ProbeTarget,
    phases: &mut ProbePhases,
  ) -> std::result::Result<u16, (&'static str, String)> {
    let url = Url::parse(&target.url).map_err(|error| ("invalid_config", error.to_string()))?;
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
      return Err(("invalid_config", "target must use http or https".to_string()));
    }

    let host = url
      .host_str()
      .ok_or_else(|| ("invalid_config", "target host is missing".to_string()))?;
    let port = url.port_or_known_default().ok_or_else(|| {
      ("invalid_config", "target port is missing and no default exists".to_string())
    })?;

    let dns_started = Instant::now();
    let mut addrs: Vec<SocketAddr> = lookup_host((host, port))
      .await
      .map_err(|error| ("dns_failed", error.to_string()))?
      .collect();
    phases.dns_ms = Some(dns_started.elapsed().as_millis() as u64);

    let addr = addrs
      .pop()
      .ok_or_else(|| ("dns_failed", "no address returned".to_string()))?;

    let tcp_started = Instant::now();
    let stream = TcpStream::connect(addr)
      .await
      .map_err(|error| ("tcp_failed", error.to_string()))?;
    phases.tcp_ms = Some(tcp_started.elapsed().as_millis() as u64);

    let path = if let Some(query) = url.query() {
      format!("{}?{}", normalized_path(url.path()), query)
    } else {
      normalized_path(url.path()).to_string()
    };

    if scheme == "https" {
      let tls_started = Instant::now();
      let connector = native_tls::TlsConnector::new()
        .map_err(|error| ("tls_failed", error.to_string()))?;
      let connector = TlsConnector::from(connector);
      let mut stream = connector
        .connect(host, stream)
        .await
        .map_err(|error| ("tls_failed", error.to_string()))?;
      phases.tls_ms = Some(tls_started.elapsed().as_millis() as u64);
      write_head_request(&mut stream, host, &path).await?;
      read_status_code(&mut stream, phases).await
    } else {
      let mut stream = stream;
      write_head_request(&mut stream, host, &path).await?;
      read_status_code(&mut stream, phases).await
    }
  }
}

fn failed_probe(
  started_at: chrono::DateTime<Utc>,
  ended_at: chrono::DateTime<Utc>,
  duration_ms: u64,
  phases: ProbePhases,
  code: impl Into<String>,
  message: impl Into<String>,
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
      code: code.into(),
      message: message.into(),
    }),
  }
}

async fn write_head_request<S>(stream: &mut S, host: &str, path: &str) -> std::result::Result<(), (&'static str, String)>
where
  S: AsyncWriteExt + Unpin,
{
  let request = format!(
    "HEAD {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: tauri-plugin-net-watcher/0.1\r\nConnection: close\r\n\r\n"
  );
  stream
    .write_all(request.as_bytes())
    .await
    .map_err(|error| ("http_failed", error.to_string()))
}

async fn read_status_code<S>(
  stream: &mut S,
  phases: &mut ProbePhases,
) -> std::result::Result<u16, (&'static str, String)>
where
  S: AsyncReadExt + Unpin,
{
  let http_started = Instant::now();
  let mut buf = vec![0_u8; 1024];
  let read = stream
    .read(&mut buf)
    .await
    .map_err(|error| ("http_failed", error.to_string()))?;
  phases.http_ms = Some(http_started.elapsed().as_millis() as u64);
  let response = String::from_utf8_lossy(&buf[..read]);
  let status = response
    .lines()
    .next()
    .and_then(|line| line.split_whitespace().nth(1))
    .and_then(|value| value.parse::<u16>().ok())
    .ok_or_else(|| ("http_failed", "missing HTTP status line".to_string()))?;

  if (200..400).contains(&status) {
    Ok(status)
  } else {
    Err(("http_status_error", format!("unexpected HTTP status {status}")))
  }
}

fn normalized_path(path: &str) -> &str {
  if path.is_empty() {
    "/"
  } else {
    path
  }
}
```

在 `src/lib.rs` 添加：

```rust
mod probe;
```

- [ ] **Step 4: 运行测试并确认通过**

Run:

```powershell
cargo test probe
```

Expected: PASS。

- [ ] **Step 5: 提交探测器**

Run:

```powershell
git add tauri-plugin-net-watcher/Cargo.toml tauri-plugin-net-watcher/src/probe.rs tauri-plugin-net-watcher/src/lib.rs
git commit -m "feat: add http quality prober"
```

---

### Task 6: 系统网络快照读取

**Files:**
- Create: `src/network.rs`
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Test: `src/network.rs`

- [ ] **Step 1: 写接口分类测试**

在 `src/network.rs` 添加：

```rust
#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn classifies_common_interface_names() {
    assert_eq!(classify_interface("Wi-Fi"), InterfaceType::Wifi);
    assert_eq!(classify_interface("en0"), InterfaceType::Wifi);
    assert_eq!(classify_interface("Ethernet"), InterfaceType::Ethernet);
    assert_eq!(classify_interface("eth0"), InterfaceType::Ethernet);
    assert_eq!(classify_interface("utun0"), InterfaceType::Vpn);
    assert_eq!(classify_interface("lo0"), InterfaceType::Loopback);
  }
}
```

- [ ] **Step 2: 添加依赖并运行失败测试**

修改 `Cargo.toml`：

```toml
get_if_addrs = "0.5"
```

Run:

```powershell
cargo test network
```

Expected: FAIL，错误包含 `classify_interface` 未定义。

- [ ] **Step 3: 实现网络快照读取**

创建 `src/network.rs`：

```rust
use get_if_addrs::{get_if_addrs, IfAddr};

use crate::{models::*, Result};

pub fn read_network_snapshot(include_mac_address: bool) -> Result<NetworkSnapshot> {
  let mut interfaces = Vec::new();

  for iface in get_if_addrs()? {
    let id = format!("if_{}", sanitize_id(&iface.name));
    let mut addresses = InterfaceAddresses::default();

    match iface.addr {
      IfAddr::V4(addr) => addresses.ipv4.push(addr.ip.to_string()),
      IfAddr::V6(addr) => addresses.ipv6.push(addr.ip.to_string()),
    }

    let is_loopback = iface.is_loopback();
    let interface_type = classify_interface(&iface.name);
    let status = if is_loopback {
      InterfaceStatus::Down
    } else {
      InterfaceStatus::Up
    };

    interfaces.push(NetworkInterface {
      id,
      name: iface.name.clone(),
      display_name: iface.name,
      interface_type,
      status,
      is_primary: false,
      addresses: InterfaceAddresses {
        mac: if include_mac_address { None } else { None },
        ..addresses
      },
      gateway: None,
      dns_servers: Vec::new(),
    });
  }

  mark_primary(&mut interfaces);
  let primary_interface_id = interfaces
    .iter()
    .find(|iface| iface.is_primary)
    .map(|iface| iface.id.clone());

  Ok(NetworkSnapshot {
    primary_interface_id,
    interfaces,
  })
}

pub fn has_available_interface(snapshot: &NetworkSnapshot) -> bool {
  snapshot.interfaces.iter().any(|iface| {
    iface.status == InterfaceStatus::Up
      && iface.interface_type != InterfaceType::Loopback
      && (!iface.addresses.ipv4.is_empty() || !iface.addresses.ipv6.is_empty())
  })
}

pub fn classify_interface(name: &str) -> InterfaceType {
  let lower = name.to_ascii_lowercase();
  if lower.starts_with("lo") || lower.contains("loopback") {
    InterfaceType::Loopback
  } else if lower.contains("wi-fi")
    || lower.contains("wifi")
    || lower.contains("wireless")
    || lower == "en0"
  {
    InterfaceType::Wifi
  } else if lower.starts_with("utun")
    || lower.starts_with("tun")
    || lower.starts_with("tap")
    || lower.contains("vpn")
  {
    InterfaceType::Vpn
  } else if lower.contains("ethernet") || lower.starts_with("eth") || lower.starts_with("en") {
    InterfaceType::Ethernet
  } else {
    InterfaceType::Unknown
  }
}

fn mark_primary(interfaces: &mut [NetworkInterface]) {
  if let Some(primary) = interfaces.iter_mut().find(|iface| {
    iface.status == InterfaceStatus::Up
      && iface.interface_type != InterfaceType::Loopback
      && !iface.addresses.ipv4.is_empty()
  }) {
    primary.is_primary = true;
  }
}

fn sanitize_id(name: &str) -> String {
  name
    .chars()
    .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '_' })
    .collect()
}
```

在 `src/lib.rs` 添加：

```rust
mod network;
```

- [ ] **Step 4: 运行测试并确认通过**

Run:

```powershell
cargo test network
```

Expected: PASS。

- [ ] **Step 5: 提交网络快照模块**

Run:

```powershell
git add tauri-plugin-net-watcher/Cargo.toml tauri-plugin-net-watcher/src/network.rs tauri-plugin-net-watcher/src/lib.rs
git commit -m "feat: add system network snapshot reader"
```

---

### Task 7: 桌面 watcher 和 Tauri 命令

**Files:**
- Replace: `src/desktop.rs`
- Replace: `src/commands.rs`
- Modify: `src/lib.rs`
- Modify: `build.rs`
- Modify: `permissions/default.toml`
- Test: `cargo test`

- [ ] **Step 1: 更新命令列表和权限**

替换 `build.rs` 中命令列表：

```rust
const COMMANDS: &[&str] = &["get_snapshot", "start_watching", "stop_watching", "get_config"];
```

替换 `permissions/default.toml`：

```toml
[default]
description = "Default permissions for the net-watcher plugin"
permissions = [
  "allow-get-snapshot",
  "allow-start-watching",
  "allow-stop-watching",
  "allow-get-config"
]
```

- [ ] **Step 2: 实现命令入口**

替换 `src/commands.rs`：

```rust
use tauri::{command, AppHandle, Runtime};

use crate::{
  config::{NetWatcherConfig, StartWatchingOptions},
  models::NetWatcherSnapshot,
  NetWatcherExt, Result,
};

#[command]
pub(crate) async fn get_snapshot<R: Runtime>(app: AppHandle<R>) -> Result<NetWatcherSnapshot> {
  app.net_watcher().get_snapshot().await
}

#[command]
pub(crate) async fn start_watching<R: Runtime>(
  app: AppHandle<R>,
  options: Option<StartWatchingOptions>,
) -> Result<()> {
  app.net_watcher().start_watching(options.unwrap_or_default()).await
}

#[command]
pub(crate) async fn stop_watching<R: Runtime>(app: AppHandle<R>) -> Result<()> {
  app.net_watcher().stop_watching().await
}

#[command]
pub(crate) async fn get_config<R: Runtime>(app: AppHandle<R>) -> Result<NetWatcherConfig> {
  app.net_watcher().get_config().await
}
```

- [ ] **Step 3: 实现桌面 watcher**

替换 `src/desktop.rs`：

```rust
use std::{sync::Arc, time::Duration};

use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Emitter, Runtime};
use tokio::{
  sync::{Mutex, RwLock},
  task::JoinHandle,
};

use crate::{
  config::{NetWatcherConfig, StartWatchingOptions},
  models::*,
  network::{has_available_interface, read_network_snapshot},
  probe::HttpProber,
  state::{evaluate_state, StateConfig},
  stats::RollingWindow,
  Error, Result,
};

const SNAPSHOT_EVENT: &str = "net-watcher://snapshot-updated";

pub fn init<R: Runtime, C: DeserializeOwned>(
  app: &AppHandle<R>,
  _api: PluginApi<R, C>,
  config: NetWatcherConfig,
) -> Result<NetWatcher<R>> {
  let watcher = NetWatcher::new(app.clone(), config.clone());
  if config.auto_start {
    watcher.start_background(StartWatchingOptions::default())?;
  }
  Ok(watcher)
}

pub struct NetWatcher<R: Runtime> {
  app: AppHandle<R>,
  config: Arc<RwLock<NetWatcherConfig>>,
  snapshot: Arc<RwLock<NetWatcherSnapshot>>,
  task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl<R: Runtime> NetWatcher<R> {
  pub fn new(app: AppHandle<R>, config: NetWatcherConfig) -> Self {
    Self {
      app,
      snapshot: Arc::new(RwLock::new(NetWatcherSnapshot::initial(env!("CARGO_PKG_VERSION")))),
      config: Arc::new(RwLock::new(config)),
      task: Arc::new(Mutex::new(None)),
    }
  }

  pub async fn get_snapshot(&self) -> Result<NetWatcherSnapshot> {
    Ok(self.snapshot.read().await.clone())
  }

  pub async fn get_config(&self) -> Result<NetWatcherConfig> {
    Ok(self.config.read().await.clone())
  }

  pub async fn start_watching(&self, options: StartWatchingOptions) -> Result<()> {
    self.start_background(options)
  }

  pub async fn stop_watching(&self) -> Result<()> {
    let mut task = self.task.lock().await;
    if let Some(handle) = task.take() {
      handle.abort();
      Ok(())
    } else {
      Err(Error::not_watching())
    }
  }

  fn start_background(&self, options: StartWatchingOptions) -> Result<()> {
    let app = self.app.clone();
    let config = self.config.clone();
    let snapshot = self.snapshot.clone();
    let task = self.task.clone();

    tauri::async_runtime::spawn(async move {
      let mut guard = task.lock().await;
      if guard.is_some() {
        return;
      }

      let session_config = {
        let base = config.read().await.clone();
        base.with_runtime_options(options)
      };
      if session_config.validate().is_err() {
        return;
      }

      let handle = tauri::async_runtime::spawn(run_loop(app, snapshot, session_config));
      *guard = Some(handle);
    });

    Ok(())
  }
}

async fn run_loop<R: Runtime>(
  app: AppHandle<R>,
  snapshot: Arc<RwLock<NetWatcherSnapshot>>,
  config: NetWatcherConfig,
) {
  let mut window = RollingWindow::new(config.window_size);
  let prober = HttpProber::new(config.timeout_ms);
  let target = ProbeTarget {
    target_type: ProbeTargetType::Http,
    url: config.target.clone(),
  };

  loop {
    let network = read_network_snapshot(config.include_mac_address).unwrap_or_default();
    let probe = prober.probe(&target).await;
    window.push(probe);
    let summary = window.summary();
    let previous = snapshot.read().await.state.overall.clone();
    let state = evaluate_state(
      has_available_interface(&network),
      &summary,
      &StateConfig {
        degraded_failure_rate: config.degraded_failure_rate,
        degraded_p95_latency_ms: config.degraded_p95_latency_ms,
        offline_consecutive_failures: config.offline_consecutive_failures,
      },
    );
    let current = state.overall.clone();

    let next = NetWatcherSnapshot {
      meta: SnapshotMeta {
        snapshot_id: format!("nw_{}", uuid::Uuid::new_v4()),
        timestamp: chrono::Utc::now(),
        platform: std::env::consts::OS.to_string(),
        plugin_version: env!("CARGO_PKG_VERSION").to_string(),
      },
      state,
      network,
      quality: QualitySnapshot {
        config: QualityConfigSnapshot {
          interval_ms: config.interval_ms,
          window_size: config.window_size,
          timeout_ms: config.timeout_ms,
        },
        target: target.clone(),
        current_probe: window.latest(),
        summary,
      },
      changes: SnapshotChanges {
        has_changes: previous != current,
        previous_overall: Some(previous),
        current_overall: current,
        changed_fields: Vec::new(),
      },
    };

    *snapshot.write().await = next.clone();
    let _ = app.emit(SNAPSHOT_EVENT, next);
    tokio::time::sleep(Duration::from_millis(config.interval_ms)).await;
  }
}
```

- [ ] **Step 4: 更新插件初始化**

修改 `src/lib.rs`：

```rust
Builder::new("net-watcher")
  .invoke_handler(tauri::generate_handler![
    commands::get_snapshot,
    commands::start_watching,
    commands::stop_watching,
    commands::get_config
  ])
  .setup(|app, api| {
    let config = api.config().clone().try_into().unwrap_or_default();
    #[cfg(mobile)]
    let net_watcher = mobile::init(app, api, config)?;
    #[cfg(desktop)]
    let net_watcher = desktop::init(app, api, config)?;
    app.manage(net_watcher);
    Ok(())
  })
  .build()
```

如果 `api.config().clone().try_into()` 的类型不匹配，改成用 `serde_json::from_value(api.config().clone())`，并在失败时使用 `NetWatcherConfig::default()`。

- [ ] **Step 5: 运行全量 Rust 测试**

Run:

```powershell
cargo test
cargo check
```

Expected: PASS。

- [ ] **Step 6: 提交命令和 watcher**

Run:

```powershell
git add tauri-plugin-net-watcher/src/desktop.rs tauri-plugin-net-watcher/src/commands.rs tauri-plugin-net-watcher/src/lib.rs tauri-plugin-net-watcher/build.rs tauri-plugin-net-watcher/permissions/default.toml
git commit -m "feat: wire net watcher commands"
```

---

### Task 8: TypeScript API

**Files:**
- Replace: `guest-js/index.ts`
- Test: `pnpm build`

- [ ] **Step 1: 替换前端 API**

替换 `guest-js/index.ts`：

```ts
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

export type OverallState = 'unknown' | 'offline' | 'localOnly' | 'degraded' | 'online'

export interface StartWatchingOptions {
  target?: string
  intervalMs?: number
  timeoutMs?: number
}

export interface NetWatcherConfig {
  autoStart: boolean
  target: string
  intervalMs: number
  timeoutMs: number
  windowSize: number
  degradedFailureRate: number
  degradedP95LatencyMs: number
  offlineConsecutiveFailures: number
  includeMacAddress: boolean
}

export interface NetWatcherSnapshot {
  meta: {
    snapshotId: string
    timestamp: string
    platform: string
    pluginVersion: string
  }
  state: {
    overall: OverallState
    network: 'unknown' | 'disconnected' | 'connected'
    quality: 'unknown' | 'unreachable' | 'unstable' | 'stable'
    score: number
    reason: string
  }
  network: {
    primaryInterfaceId?: string | null
    interfaces: Array<{
      id: string
      name: string
      displayName: string
      type: 'wifi' | 'ethernet' | 'vpn' | 'loopback' | 'unknown'
      status: 'up' | 'down'
      isPrimary: boolean
      addresses: {
        ipv4: string[]
        ipv6: string[]
        mac?: string | null
      }
      gateway?: string | null
      dnsServers: string[]
    }>
  }
  quality: {
    config: {
      intervalMs: number
      windowSize: number
      timeoutMs: number
    }
    target: {
      type: 'http'
      url: string
    }
    currentProbe?: {
      id: string
      status: 'success' | 'failed'
      startedAt: string
      endedAt: string
      durationMs: number
      phases: {
        dnsMs?: number | null
        tcpMs?: number | null
        tlsMs?: number | null
        httpMs?: number | null
      }
      http?: {
        statusCode: number
      } | null
      error?: {
        code: string
        message: string
      } | null
    } | null
    summary: {
      sampleCount: number
      successCount: number
      failureCount: number
      failureRate: number
      latencyMs: {
        avg: number
        min: number
        max: number
        p95: number
      }
      jitterMs: number
      consecutiveFailures: number
      lastSuccessAt?: string | null
      lastFailureAt?: string | null
      lastFailureReason?: string | null
    }
  }
  changes: {
    hasChanges: boolean
    previousOverall?: OverallState | null
    currentOverall: OverallState
    changedFields: string[]
  }
}

const SNAPSHOT_EVENT = 'net-watcher://snapshot-updated'

export async function getSnapshot(): Promise<NetWatcherSnapshot> {
  return await invoke<NetWatcherSnapshot>('plugin:net-watcher|get_snapshot')
}

export async function startWatching(options?: StartWatchingOptions): Promise<void> {
  await invoke('plugin:net-watcher|start_watching', { options })
}

export async function stopWatching(): Promise<void> {
  await invoke('plugin:net-watcher|stop_watching')
}

export async function getConfig(): Promise<NetWatcherConfig> {
  return await invoke<NetWatcherConfig>('plugin:net-watcher|get_config')
}

export async function onSnapshotUpdated(
  handler: (snapshot: NetWatcherSnapshot) => void,
): Promise<UnlistenFn> {
  return await listen<NetWatcherSnapshot>(SNAPSHOT_EVENT, (event) => handler(event.payload))
}
```

- [ ] **Step 2: 构建 JS API**

Run:

```powershell
pnpm install
pnpm build
```

Expected: PASS，并生成 `dist-js`。

- [ ] **Step 3: 提交 JS API**

Run:

```powershell
git add tauri-plugin-net-watcher/guest-js/index.ts tauri-plugin-net-watcher/package.json tauri-plugin-net-watcher/pnpm-lock.yaml
git commit -m "feat: add net watcher javascript api"
```

---

### Task 9: 示例应用和 README

**Files:**
- Modify: `examples/tauri-app/src/App.svelte`
- Modify: `examples/tauri-app/src-tauri/tauri.conf.json`
- Replace: `README.md`

- [ ] **Step 1: 配置示例插件**

在 `examples/tauri-app/src-tauri/tauri.conf.json` 添加：

```json
{
  "plugins": {
    "net-watcher": {
      "autoStart": false,
      "target": "https://www.apple.com/library/test/success.html",
      "intervalMs": 10000,
      "timeoutMs": 3000
    }
  }
}
```

保留该文件现有其他配置，不删除已有 `app`、`bundle`、`identifier` 等字段。

- [ ] **Step 2: 更新示例 UI**

把 `examples/tauri-app/src/App.svelte` 改成调用 `getSnapshot`、`startWatching`、`stopWatching` 和 `onSnapshotUpdated`，页面至少展示：

```svelte
<script lang="ts">
  import {
    getSnapshot,
    onSnapshotUpdated,
    startWatching,
    stopWatching,
    type NetWatcherSnapshot,
  } from 'tauri-plugin-net-watcher-api'

  let snapshot: NetWatcherSnapshot | null = null
  let error = ''
  let unlisten: (() => void) | null = null

  async function refresh() {
    snapshot = await getSnapshot()
  }

  async function start() {
    error = ''
    try {
      await startWatching()
      unlisten = await onSnapshotUpdated((next) => {
        snapshot = next
      })
      await refresh()
    } catch (err) {
      error = String(err)
    }
  }

  async function stop() {
    error = ''
    try {
      await stopWatching()
      unlisten?.()
      unlisten = null
      await refresh()
    } catch (err) {
      error = String(err)
    }
  }
</script>

<main class="container">
  <h1>Net Watcher</h1>
  <div class="toolbar">
    <button on:click={start}>Start</button>
    <button on:click={stop}>Stop</button>
    <button on:click={refresh}>Refresh</button>
  </div>
  {#if error}
    <p class="error">{error}</p>
  {/if}
  {#if snapshot}
    <section>
      <h2>{snapshot.state.overall}</h2>
      <p>Score: {snapshot.state.score}</p>
      <p>Reason: {snapshot.state.reason}</p>
      <p>Target: {snapshot.quality.target.url}</p>
      <p>Failure rate: {snapshot.quality.summary.failureRate}</p>
      <p>P95 latency: {snapshot.quality.summary.latencyMs.p95}ms</p>
    </section>
  {/if}
</main>
```

- [ ] **Step 3: 更新 README**

替换 `README.md`：

```markdown
# Tauri Plugin Net Watcher

`tauri-plugin-net-watcher` 是一个 Tauri v2 桌面插件，用于监控 Windows 和 macOS 的网络状态与网络质量。

## 功能

- 获取当前网络快照。
- 监听网络快照更新事件。
- 通过 HTTP/HTTPS 目标探测网络质量。
- 输出 `unknown`、`offline`、`localOnly`、`degraded`、`online` 状态。

## 配置

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

## 前端使用

```ts
import {
  getSnapshot,
  onSnapshotUpdated,
  startWatching,
  stopWatching,
} from 'tauri-plugin-net-watcher-api'

await startWatching()

const snapshot = await getSnapshot()
console.log(snapshot.state.overall)

const unlisten = await onSnapshotUpdated((next) => {
  console.log(next.state.overall, next.state.score)
})

await stopWatching()
unlisten()
```
```

- [ ] **Step 4: 验证示例构建**

Run:

```powershell
cd examples\tauri-app
pnpm install
pnpm tauri build --debug
```

Expected: PASS。

- [ ] **Step 5: 提交示例和文档**

Run:

```powershell
git add tauri-plugin-net-watcher/examples/tauri-app/src/App.svelte tauri-plugin-net-watcher/examples/tauri-app/src-tauri/tauri.conf.json tauri-plugin-net-watcher/README.md
git commit -m "docs: add net watcher usage example"
```

---

### Task 10: 最终验证

**Files:**
- No source edits expected.

- [ ] **Step 1: 运行 Rust 测试**

Run:

```powershell
cargo test
cargo check
```

Expected: PASS。

- [ ] **Step 2: 运行 JS 构建**

Run:

```powershell
pnpm build
```

Expected: PASS。

- [ ] **Step 3: 检查权限文件生成结果**

Run:

```powershell
Get-ChildItem -Recurse -Path permissions
```

Expected: 能看到 `default.toml`，并且命令权限与 `build.rs` 中命令列表一致。

- [ ] **Step 4: 手动验证示例应用**

Run:

```powershell
cd examples\tauri-app
pnpm tauri dev
```

Expected:

- 点击 Start 后 `state.overall` 从 `unknown` 变为 `online`、`degraded` 或 `localOnly`。
- 断开网络后状态能变为 `offline` 或 `localOnly`。
- 恢复网络后状态能回到 `online` 或 `degraded`。
- 点击 Stop 后不再收到新的快照更新。

- [ ] **Step 5: 查看最终 diff**

Run:

```powershell
git status --short
git log --oneline -5
```

Expected: 所有实现改动已经分批提交，工作区只剩用户明确保留的未跟踪文件。

