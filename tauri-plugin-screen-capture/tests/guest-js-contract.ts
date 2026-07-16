import {
  createAnnotationController,
  onCaptureSessionEnded,
  type AnnotationElement,
  type CaptureSessionEndedEvent,
  type StartCaptureOptions,
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

const captureOptions: StartCaptureOptions = {
  sourceId: "display:1",
  sourceKind: "display",
  annotations: { enabled: true },
}

const pen: AnnotationElement = {
  id: "pen-1",
  kind: "pen",
  points: [{ x: 0.25, y: 0.5 }],
  color: { red: 255, green: 0, blue: 0, alpha: 255 },
  width: 0.01,
}

const annotations = createAnnotationController("session-1")
void annotations.setVisible(true)
void annotations.beginElement(pen)
void annotations.updateElement({ ...pen, points: [...pen.points, { x: 0.5, y: 0.5 }] })
void annotations.commitElement(pen)
void annotations.eraseAt({ x: 0.5, y: 0.5 })
void annotations.removeElement(pen.id)
void annotations.undo()
void annotations.redo()
void annotations.clear()
void captureOptions
