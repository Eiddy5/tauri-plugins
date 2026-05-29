# Tauri Plugin screen-capture

## Publisher selection

`startCapture` defaults to the existing WebRTC loopback publisher:

```ts
await startCapture({
  sourceId: "display:1",
  sourceKind: "display",
})
```

Agora publishing is represented in the public API so the screen capture
pipeline can switch publishers without changing the macOS or Windows capture
backends:

```ts
await startCapture({
  sourceId: "display:1",
  sourceKind: "display",
  publisher: {
    kind: "agora",
    agora: {
      appId: "your-agora-app-id",
      channel: "meeting-screen-share",
      uid: 42,
      token: "optional-token",
    },
  },
})
```

The native Agora SDK adapter is not linked yet. Selecting `kind: "agora"` now
returns a structured `publisherUnsupported` error before platform capture is
started. The next implementation step is to bind the Agora Native SDK and map
`VideoFrame` values to Agora external video frames.

Internally, `AgoraPublisher` already exposes an injectable `AgoraVideoSink`
boundary. The capture side currently produces tightly packed BGRA frames on
macOS and Windows, and the Agora boundary maps those frames to
`AgoraExternalVideoFrame` with microsecond timestamps. A real SDK sink should
initialize the Agora engine, join the configured channel, enable the external
video source before publishing, and push each mapped frame through the native
Agora external-video-frame API.
