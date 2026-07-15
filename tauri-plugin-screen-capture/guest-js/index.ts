import { invoke } from '@tauri-apps/api/core'
import { listen, type Event, type UnlistenFn } from '@tauri-apps/api/event'

export type CaptureSourceKind = 'display' | 'window'
export type PublisherKind = 'webRtcLoopback' | 'agora'
export type CaptureStatus =
  | 'idle'
  | 'starting'
  | 'capturing'
  | 'publishing'
  | 'paused'
  | 'stopping'
  | 'stopped'
  | 'failed'

export interface Capabilities {
  platform: string
  supportsDisplayCapture: boolean
  supportsWindowCapture: boolean
  supportsThumbnails: boolean
  supportsCursorCapture: boolean
  supportsWebrtc: boolean
}

export interface CaptureSource {
  id: string
  kind: CaptureSourceKind
  name: string
  title?: string | null
  appName?: string | null
  pid?: number | null
  width: number
  height: number
  scaleFactor: number
  isPrimary: boolean
  thumbnailBase64?: string | null
  filteredReason?: string | null
}

export interface ListSourcesOptions {
  kinds?: CaptureSourceKind[]
  includeThumbnails?: boolean
  includeCurrentApp?: boolean
  includeSystemUi?: boolean
  debugRawSources?: boolean
}

export interface AgoraPublisherOptions {
  appId: string
  channel: string
  uid?: number | null
  token?: string | null
}

export interface PublisherOptions {
  kind: PublisherKind
  agora?: AgoraPublisherOptions | null
}

export interface StartCaptureOptions {
  sourceId: string
  sourceKind: CaptureSourceKind
  fps?: number
  width?: number
  height?: number
  captureCursor?: boolean
  publisher?: PublisherOptions | null
}

export interface CaptureErrorPayload {
  code: string
  message: string
  recoverable: boolean
  details?: unknown
}

export interface CaptureSession {
  sessionId: string
  sourceId: string
  sourceKind: CaptureSourceKind
  status: CaptureStatus
  startedAtMs: number
  lastError?: CaptureErrorPayload | null
}

export interface CaptureSessionEndedEvent {
  sessionId: string
  error: CaptureErrorPayload
}

export const CAPTURE_SESSION_ENDED_EVENT = 'screen-capture://session-ended'

export interface CaptureStats {
  framesCaptured: number
  framesPublished: number
  framesDropped: number
  fps: number
  bitrateKbps: number
  started: boolean
}

export interface WebRtcOffer {
  sdp: string
  sdpType: string
}

export interface WebRtcAnswer {
  sdp: string
  sdpType: string
}

export interface WebRtcIceCandidate {
  candidate: string
  sdpMid?: string | null
  sdpMlineIndex?: number | null
}

const command = (name: string) => `plugin:screen-capture|${name}`

export function getCapabilities() {
  return invoke<Capabilities>(command('get_capabilities'))
}

export function checkPermission() {
  return invoke(command('check_permission'))
}

export function requestPermission() {
  return invoke(command('request_permission'))
}

export function listSources(options: ListSourcesOptions = {}) {
  return invoke<CaptureSource[]>(command('list_sources'), { options })
}

export function startCapture(options: StartCaptureOptions) {
  return invoke<CaptureSession>(command('start_capture'), { options })
}

export function pauseCapture(sessionId: string) {
  return invoke<void>(command('pause_capture'), { sessionId })
}

export function resumeCapture(sessionId: string) {
  return invoke<void>(command('resume_capture'), { sessionId })
}

export function stopCapture(sessionId: string) {
  return invoke<void>(command('stop_capture'), { sessionId })
}

export function getCaptureSession(sessionId: string) {
  return invoke<CaptureSession>(command('get_capture_session'), { sessionId })
}

export function getCaptureStats(sessionId: string) {
  return invoke<CaptureStats>(command('get_capture_stats'), { sessionId })
}

export function onCaptureSessionEnded(
  handler: (event: Event<CaptureSessionEndedEvent>) => void,
): Promise<UnlistenFn> {
  return listen<CaptureSessionEndedEvent>(CAPTURE_SESSION_ENDED_EVENT, handler)
}

export function createWebRtcOffer(sessionId: string) {
  return invoke<WebRtcOffer>(command('create_webrtc_offer'), { sessionId })
}

export function acceptWebRtcAnswer(sessionId: string, answer: WebRtcAnswer) {
  return invoke<void>(command('accept_webrtc_answer'), { sessionId, answer })
}

export function addWebRtcIceCandidate(sessionId: string, candidate: WebRtcIceCandidate) {
  return invoke<void>(command('add_webrtc_ice_candidate'), { sessionId, candidate })
}
