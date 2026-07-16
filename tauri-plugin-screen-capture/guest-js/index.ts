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
  supportsAnnotations: boolean
}

export type AnnotationElementKind = 'pen' | 'line' | 'rectangle' | 'ellipse' | 'arrow'

export interface AnnotationPoint {
  x: number
  y: number
}

export interface AnnotationColor {
  red: number
  green: number
  blue: number
  alpha: number
}

export interface AnnotationElement {
  id: string
  kind: AnnotationElementKind
  points: AnnotationPoint[]
  color: AnnotationColor
  /** Stroke width normalized against the shorter video dimension. */
  width: number
}

export interface AnnotationDocument {
  visible: boolean
  elements: AnnotationElement[]
}

export interface AnnotationOptions {
  /** Reserves a compositing-capable capture path for this session. */
  enabled?: boolean
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
  annotations?: AnnotationOptions | null
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

export function getAnnotationDocument(sessionId: string) {
  return invoke<AnnotationDocument>(command('get_annotation_document'), { sessionId })
}

export function setAnnotationDocument(sessionId: string, document: AnnotationDocument) {
  return invoke<void>(command('set_annotation_document'), { sessionId, document })
}

export interface AnnotationController {
  readonly sessionId: string
  readonly document: AnnotationDocument
  refresh(): Promise<AnnotationDocument>
  setVisible(visible: boolean): Promise<void>
  beginElement(element: AnnotationElement): Promise<void>
  updateElement(element: AnnotationElement): Promise<void>
  commitElement(element: AnnotationElement): Promise<void>
  eraseAt(point: AnnotationPoint, radius?: number): Promise<void>
  removeElement(elementId: string): Promise<void>
  clear(): Promise<void>
  undo(): Promise<void>
  redo(): Promise<void>
}

export function createAnnotationController(sessionId: string): AnnotationController {
  let current: AnnotationDocument = { visible: false, elements: [] }
  let draftBase: AnnotationDocument | null = null
  const undoStack: AnnotationDocument[] = []
  const redoStack: AnnotationDocument[] = []
  let pendingWrite: AnnotationDocument | null = null
  let flushPromise: Promise<void> | null = null

  const snapshot = (document: AnnotationDocument): AnnotationDocument => ({
    visible: document.visible,
    elements: document.elements.map((element) => ({
      ...element,
      points: element.points.map((point) => ({ ...point })),
      color: { ...element.color },
    })),
  })

  const publish = (document: AnnotationDocument): Promise<void> => {
    current = snapshot(document)
    pendingWrite = snapshot(current)
    if (!flushPromise) {
      flushPromise = (async () => {
        await Promise.resolve()
        while (pendingWrite) {
          const next = pendingWrite
          pendingWrite = null
          await setAnnotationDocument(sessionId, next)
        }
      })().finally(() => {
        flushPromise = null
      })
    }
    return flushPromise
  }

  const upsert = (document: AnnotationDocument, element: AnnotationElement): AnnotationDocument => {
    const index = document.elements.findIndex((candidate) => candidate.id === element.id)
    const elements = document.elements.slice()
    if (index === -1) elements.push(element)
    else elements[index] = element
    return { ...document, elements }
  }

  const remember = (before: AnnotationDocument) => {
    undoStack.push(snapshot(before))
    redoStack.length = 0
  }

  const pointDistanceToSegment = (
    point: AnnotationPoint,
    start: AnnotationPoint,
    end: AnnotationPoint,
  ) => {
    const deltaX = end.x - start.x
    const deltaY = end.y - start.y
    const lengthSquared = deltaX * deltaX + deltaY * deltaY
    if (lengthSquared === 0) return Math.hypot(point.x - start.x, point.y - start.y)
    const progress = Math.max(
      0,
      Math.min(1, ((point.x - start.x) * deltaX + (point.y - start.y) * deltaY) / lengthSquared),
    )
    return Math.hypot(
      point.x - (start.x + progress * deltaX),
      point.y - (start.y + progress * deltaY),
    )
  }

  const elementSegments = (element: AnnotationElement): [AnnotationPoint, AnnotationPoint][] => {
    const [start, end] = element.points
    if (!start) return []
    if (element.kind === 'pen') {
      if (element.points.length === 1) return [[start, start]]
      return element.points.slice(1).map((point, index) => [element.points[index], point])
    }
    if (!end) return []
    if (element.kind === 'rectangle') {
      const topRight = { x: end.x, y: start.y }
      const bottomLeft = { x: start.x, y: end.y }
      return [
        [start, topRight],
        [topRight, end],
        [end, bottomLeft],
        [bottomLeft, start],
      ]
    }
    if (element.kind === 'ellipse') {
      const center = { x: (start.x + end.x) / 2, y: (start.y + end.y) / 2 }
      const radiusX = Math.abs(end.x - start.x) / 2
      const radiusY = Math.abs(end.y - start.y) / 2
      const points = Array.from({ length: 37 }, (_, index) => {
        const angle = (index / 36) * Math.PI * 2
        return { x: center.x + radiusX * Math.cos(angle), y: center.y + radiusY * Math.sin(angle) }
      })
      return points.slice(1).map((candidate, index) => [points[index], candidate])
    }
    return [[start, end]]
  }

  return {
    sessionId,
    get document() {
      return snapshot(current)
    },
    async refresh() {
      await flushPromise
      current = snapshot(await getAnnotationDocument(sessionId))
      draftBase = null
      undoStack.length = 0
      redoStack.length = 0
      return snapshot(current)
    },
    setVisible(visible) {
      draftBase = null
      return publish({ ...current, visible })
    },
    beginElement(element) {
      if (!draftBase) draftBase = snapshot(current)
      return publish(upsert(current, element))
    },
    updateElement(element) {
      if (!draftBase) draftBase = snapshot(current)
      return publish(upsert(current, element))
    },
    commitElement(element) {
      const before = draftBase ?? snapshot(current)
      draftBase = null
      remember(before)
      return publish(upsert(current, element))
    },
    eraseAt(point, radius = 0.02) {
      const element = [...current.elements].reverse().find((candidate) =>
        elementSegments(candidate).some(
          ([start, end]) =>
            pointDistanceToSegment(point, start, end) <= radius + candidate.width / 2,
        ),
      )
      if (!element) return flushPromise ?? Promise.resolve()
      draftBase = null
      remember(current)
      return publish({
        ...current,
        elements: current.elements.filter((candidate) => candidate.id !== element.id),
      })
    },
    removeElement(elementId) {
      draftBase = null
      remember(current)
      return publish({
        ...current,
        elements: current.elements.filter((element) => element.id !== elementId),
      })
    },
    clear() {
      draftBase = null
      remember(current)
      return publish({ ...current, elements: [] })
    },
    undo() {
      draftBase = null
      const previous = undoStack.pop()
      if (!previous) return flushPromise ?? Promise.resolve()
      redoStack.push(snapshot(current))
      return publish(previous)
    },
    redo() {
      draftBase = null
      const next = redoStack.pop()
      if (!next) return flushPromise ?? Promise.resolve()
      undoStack.push(snapshot(current))
      return publish(next)
    },
  }
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
