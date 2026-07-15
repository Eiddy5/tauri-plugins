import {
  onCaptureSessionEnded,
  type CaptureSessionEndedEvent,
} from "../guest-js/index"

const event: CaptureSessionEndedEvent = {
  sessionId: "session-1",
  error: {
    code: "sourceUnavailable",
    message: "被共享窗口已关闭",
    recoverable: true,
  },
}

void onCaptureSessionEnded(({ payload }) => {
  const sameEvent: CaptureSessionEndedEvent = payload
  void sameEvent
  void event
})
