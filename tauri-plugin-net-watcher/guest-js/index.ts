import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'

export type OverallState = 'unknown' | 'offline' | 'online'

export interface StartWatchingOptions {
  targets?: ReachabilityTargetConfig[]
  intervalMs?: number
  timeoutMs?: number
}

export interface NetWatcherConfig {
  autoStart: boolean
  targets: ReachabilityTargetConfig[]
  intervalMs: number
  timeoutMs: number
}

export interface ReachabilityTargetConfig {
  id: string
  url: string
}

export interface NetWatcherSnapshot {
  meta: SnapshotMeta
  state: SnapshotState
  network: NetworkSnapshot
  reachability: ReachabilitySnapshot
  changes: SnapshotChanges
}

export interface NetworkUpdatedPayload {
  snapshotId: string
  timestamp: string
  platform: string
  state: SnapshotState
  network: NetworkSnapshot
}

export interface TargetUpdatedPayload {
  snapshotId: string
  timestamp: string
  target: ReachabilityTargetSnapshot
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
  internet: InternetStatus
  reason: string
}

export type NetworkLayerState = 'unknown' | 'disconnected' | 'connected'

export interface NetworkSnapshot {
  primaryInterfaceId: string | null
  interfaces: NetworkInterface[]
  internet: InternetSnapshot
}

export interface InternetSnapshot {
  status: InternetStatus
  verified: boolean
  systemHint: InternetSystemHint
  activeProbe: InternetProbeResult | null
  captivePortal: boolean
  checkedAt: string | null
  consecutiveFailures: number
  reason: string
}

export type InternetStatus =
  | 'unknown'
  | 'available'
  | 'degraded'
  | 'unavailable'
  | 'captivePortal'

export interface InternetSystemHint {
  source: InternetHintSource
  level: InternetHintLevel
}

export type InternetHintSource = 'windowsNcsi' | 'macosReachability' | 'unavailable'

export type InternetHintLevel =
  | 'unknown'
  | 'none'
  | 'localAccess'
  | 'constrainedInternetAccess'
  | 'internetAccess'

export interface InternetProbeResult {
  status: InternetProbeStatus
  durationMs: number
  httpStatus: number | null
  error: ProbeError | null
}

export type InternetProbeStatus = 'success' | 'failed' | 'unexpectedResponse'

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

export interface ReachabilitySnapshot {
  config: ReachabilityConfigSnapshot
  targets: ReachabilityTargetSnapshot[]
}

export interface ReachabilityTargetSnapshot {
  id: string
  state: ReachabilityTargetState
  target: ProbeTarget
  currentProbe: ProbeResult | null
  summary: QualitySummary
}

export interface ReachabilityTargetState {
  reachability: ReachabilityStatus
  quality: TargetQualityState
  reason: string
}

export type ReachabilityStatus = 'unknown' | 'reachable' | 'unreachable'

export type TargetQualityState = 'unknown' | 'unstable' | 'stable'

export interface ReachabilityConfigSnapshot {
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
  changedTargetIds: string[]
}

export const NETWORK_UPDATED_EVENT = 'net-watcher://network-updated'
export const TARGET_UPDATED_EVENT = 'net-watcher://target-updated'

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

export async function onNetworkUpdated(
  handler: (payload: NetworkUpdatedPayload) => void,
): Promise<UnlistenFn> {
  return await listen<NetworkUpdatedPayload>(NETWORK_UPDATED_EVENT, (event) => {
    handler(event.payload)
  })
}

export async function onTargetUpdated(
  handler: (payload: TargetUpdatedPayload) => void,
): Promise<UnlistenFn> {
  return await listen<TargetUpdatedPayload>(TARGET_UPDATED_EVENT, (event) => {
    handler(event.payload)
  })
}
