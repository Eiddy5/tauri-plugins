import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'

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
  meta: SnapshotMeta
  state: SnapshotState
  network: NetworkSnapshot
  quality: QualitySnapshot
  changes: SnapshotChanges
}

export interface SnapshotMeta {
  snapshotId: string
  timestamp: string
  platform: string
  pluginVersion: string
}

export interface SnapshotState {
  overall: OverallState
  network: NetworkLayerState
  quality: QualityLayerState
  score: number
  reason: string
}

export type NetworkLayerState = 'unknown' | 'disconnected' | 'connected'

export type QualityLayerState = 'unknown' | 'unreachable' | 'unstable' | 'stable'

export interface NetworkSnapshot {
  primaryInterfaceId: string | null
  interfaces: NetworkInterface[]
}

export interface NetworkInterface {
  id: string
  name: string
  displayName: string
  type: InterfaceType
  status: InterfaceStatus
  isPrimary: boolean
  addresses: InterfaceAddresses
  gateway: string | null
  dnsServers: string[]
}

export type InterfaceType = 'wifi' | 'ethernet' | 'vpn' | 'loopback' | 'unknown'

export type InterfaceStatus = 'up' | 'down'

export interface InterfaceAddresses {
  ipv4: string[]
  ipv6: string[]
  mac: string | null
}

export interface QualitySnapshot {
  config: QualityConfigSnapshot
  target: ProbeTarget
  currentProbe: ProbeResult | null
  summary: QualitySummary
}

export interface QualityConfigSnapshot {
  intervalMs: number
  windowSize: number
  timeoutMs: number
}

export interface ProbeTarget {
  type: ProbeTargetType
  url: string
}

export type ProbeTargetType = 'http'

export interface ProbeResult {
  id: string
  status: ProbeStatus
  startedAt: string
  endedAt: string
  durationMs: number
  phases: ProbePhases
  http: HttpProbeResult | null
  error: ProbeError | null
}

export type ProbeStatus = 'success' | 'failed'

export interface ProbePhases {
  dnsMs: number | null
  tcpMs: number | null
  tlsMs: number | null
  httpMs: number | null
}

export interface HttpProbeResult {
  statusCode: number
}

export interface ProbeError {
  code: string
  message: string
}

export interface QualitySummary {
  sampleCount: number
  successCount: number
  failureCount: number
  failureRate: number
  latencyMs: LatencySummary
  jitterMs: number
  consecutiveFailures: number
  lastSuccessAt: string | null
  lastFailureAt: string | null
  lastFailureReason: string | null
}

export interface LatencySummary {
  avg: number
  min: number
  max: number
  p95: number
}

export interface SnapshotChanges {
  hasChanges: boolean
  previousOverall: OverallState | null
  currentOverall: OverallState
  changedFields: string[]
}

export const SNAPSHOT_EVENT = 'net-watcher://snapshot-updated'

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
