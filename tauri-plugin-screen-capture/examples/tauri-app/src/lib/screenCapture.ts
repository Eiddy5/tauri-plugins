import {
  acceptWebRtcAnswer,
  addWebRtcIceCandidate,
  createWebRtcOffer,
  type CaptureSession,
} from 'tauri-plugin-screen-capture-api'

type FrameChunk = {
  width: number
  height: number
  pixelFormat: number
  timestampNs: bigint
  totalLength: number
  chunkIndex: number
  chunkCount: number
  offset: number
  payload: Uint8Array
}

type FrameBuffer = {
  width: number
  height: number
  pixelFormat: number
  timestampNs: bigint
  totalLength: number
  received: number
  createdAt: number
  chunks: Map<number, FrameChunk>
}

const SCF2 = 'SCF2'
const MAX_PENDING_FRAMES = 2
const PENDING_FRAME_TTL_MS = 1000

export async function connectLoopbackCanvas(session: CaptureSession, canvas: HTMLCanvasElement) {
  const context = canvas.getContext('2d')
  if (!context) throw new Error('2D canvas context is unavailable')

  const pc = new RTCPeerConnection()
  attachFrameDataChannel(pc, context)

  pc.onicecandidate = (event) => {
    if (!event.candidate) return
    void addWebRtcIceCandidate(session.sessionId, {
      candidate: event.candidate.candidate,
      sdpMid: event.candidate.sdpMid,
      sdpMlineIndex: event.candidate.sdpMLineIndex ?? null,
    })
  }

  const offer = await createWebRtcOffer(session.sessionId)
  await pc.setRemoteDescription({ type: 'offer', sdp: offer.sdp })
  const answer = await pc.createAnswer()
  await pc.setLocalDescription(answer)
  await acceptWebRtcAnswer(session.sessionId, {
    sdp: answer.sdp ?? '',
    sdpType: answer.type,
  })

  return pc
}

export async function connectLoopbackVideo(
  session: CaptureSession,
  video: HTMLVideoElement,
  fallbackCanvas?: HTMLCanvasElement,
) {
  const pc = new RTCPeerConnection()

  if (fallbackCanvas) {
    const context = fallbackCanvas.getContext('2d')
    if (!context) throw new Error('2D canvas context is unavailable')
    attachFrameDataChannel(pc, context)
  }

  pc.ontrack = (event) => {
    const [stream] = event.streams
    video.srcObject = stream ?? new MediaStream([event.track])
    void video.play()
  }

  pc.onicecandidate = (event) => {
    if (!event.candidate) return
    void addWebRtcIceCandidate(session.sessionId, {
      candidate: event.candidate.candidate,
      sdpMid: event.candidate.sdpMid,
      sdpMlineIndex: event.candidate.sdpMLineIndex ?? null,
    })
  }

  const offer = await createWebRtcOffer(session.sessionId)
  await pc.setRemoteDescription({ type: 'offer', sdp: offer.sdp })
  const answer = await pc.createAnswer()
  await pc.setLocalDescription(answer)
  await acceptWebRtcAnswer(session.sessionId, {
    sdp: answer.sdp ?? '',
    sdpType: answer.type,
  })

  return pc
}

function attachFrameDataChannel(pc: RTCPeerConnection, context: CanvasRenderingContext2D) {
  const frames = new Map<string, FrameBuffer>()

  pc.ondatachannel = (event) => {
    if (event.channel.label !== 'screen-capture-frames') return
    event.channel.binaryType = 'arraybuffer'
    event.channel.onmessage = (message) => {
      if (!(message.data instanceof ArrayBuffer)) return
      renderChunk(context, frames, new Uint8Array(message.data))
    }
  }
}

function renderChunk(
  context: CanvasRenderingContext2D,
  frames: Map<string, FrameBuffer>,
  bytes: Uint8Array,
) {
  const chunk = parseChunk(bytes)
  if (!chunk) return
  prunePendingFrames(frames)

  const key = `${chunk.timestampNs}:${chunk.width}:${chunk.height}:${chunk.totalLength}`
  const frame =
    frames.get(key) ??
    {
      width: chunk.width,
      height: chunk.height,
      pixelFormat: chunk.pixelFormat,
      timestampNs: chunk.timestampNs,
      totalLength: chunk.totalLength,
      received: 0,
      createdAt: performance.now(),
      chunks: new Map<number, Uint8Array>(),
    }

  if (!frame.chunks.has(chunk.chunkIndex)) {
    frame.chunks.set(chunk.chunkIndex, chunk)
    frame.received += chunk.payload.length
  }
  frames.set(key, frame)

  if (frame.chunks.size !== chunk.chunkCount || frame.received < frame.totalLength) return
  frames.delete(key)

  const payload = new Uint8Array(frame.totalLength)
  for (const part of frame.chunks.values()) {
    payload.set(part.payload, part.offset)
  }

  drawFrame(context, frame.width, frame.height, frame.pixelFormat, payload)
}

function prunePendingFrames(frames: Map<string, FrameBuffer>) {
  const now = performance.now()
  for (const [key, frame] of frames) {
    if (now - frame.createdAt > PENDING_FRAME_TTL_MS) frames.delete(key)
  }

  while (frames.size > MAX_PENDING_FRAMES) {
    const oldestKey = frames.keys().next().value
    if (!oldestKey) break
    frames.delete(oldestKey)
  }
}

function parseChunk(bytes: Uint8Array): FrameChunk | null {
  if (bytes.length < 41 || ascii(bytes, 0, 4) !== SCF2) return null
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const payloadLength = view.getUint32(37, true)
  if (bytes.length < 41 + payloadLength) return null
  const payload = bytes.slice(41, 41 + payloadLength)
  return {
    width: view.getUint32(4, true),
    height: view.getUint32(8, true),
    pixelFormat: view.getUint8(12),
    timestampNs: view.getBigUint64(13, true),
    totalLength: view.getUint32(21, true),
    chunkIndex: view.getUint32(25, true),
    chunkCount: view.getUint32(29, true),
    offset: view.getUint32(33, true),
    payload,
  }
}

function drawFrame(
  context: CanvasRenderingContext2D,
  width: number,
  height: number,
  pixelFormat: number,
  payload: Uint8Array,
) {
  const rgba = new Uint8ClampedArray(width * height * 4)
  for (let index = 0; index < width * height; index += 1) {
    const source = index * 4
    const target = index * 4
    if (pixelFormat === 0) {
      rgba[target] = payload[source + 2]
      rgba[target + 1] = payload[source + 1]
      rgba[target + 2] = payload[source]
      rgba[target + 3] = payload[source + 3]
    } else {
      rgba[target] = payload[source]
      rgba[target + 1] = payload[source + 1]
      rgba[target + 2] = payload[source + 2]
      rgba[target + 3] = payload[source + 3]
    }
  }

  const canvas = context.canvas
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width
    canvas.height = height
  }
  context.putImageData(new ImageData(rgba, width, height), 0, 0)
}

function ascii(bytes: Uint8Array, start: number, end: number) {
  return String.fromCharCode(...bytes.slice(start, end))
}
