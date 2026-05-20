import {
  acceptWebRtcAnswer,
  addWebRtcIceCandidate,
  createWebRtcOffer,
} from "tauri-plugin-screen-capture-api"

export async function connectVideo(session, video) {
  const pc = new RTCPeerConnection()

  pc.ontrack = (event) => {
    const [stream] = event.streams
    console.info("[screen-capture] WebRTC track received", {
      kind: event.track.kind,
      readyState: event.track.readyState,
      streamId: stream?.id ?? null,
    })
    video.srcObject = stream ?? new MediaStream([event.track])
    void video.play().catch((error) => {
      console.error("[screen-capture] video playback failed", error)
    })
  }

  video.onloadedmetadata = () => {
    console.info("[screen-capture] video metadata loaded", {
      videoWidth: video.videoWidth,
      videoHeight: video.videoHeight,
    })
  }

  bindIceCandidates(pc, session)
  await answerOffer(pc, session)

  return pc
}

function bindIceCandidates(pc, session) {
  pc.onicecandidate = (event) => {
    if (!event.candidate) return
    void addWebRtcIceCandidate(session.sessionId, {
      candidate: event.candidate.candidate,
      sdpMid: event.candidate.sdpMid,
      sdpMlineIndex: event.candidate.sdpMLineIndex ?? null,
    })
  }
}

async function answerOffer(pc, session) {
  const offer = await createWebRtcOffer(session.sessionId)
  await pc.setRemoteDescription({ type: "offer", sdp: offer.sdp })
  const answer = await pc.createAnswer()
  await pc.setLocalDescription(answer)
  await acceptWebRtcAnswer(session.sessionId, {
    sdp: answer.sdp ?? "",
    sdpType: answer.type,
  })
}
