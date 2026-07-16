# Cross-platform screen-share annotations

## Goal

Add DingTalk-style annotations on top of an active screen share. Published viewers must see the captured screen and annotations as one video frame. Windows and macOS callers use the same public API and wire model.

## Public contract

- `StartCaptureOptions.annotations.enabled` opts a capture session into annotation compositing.
- `AnnotationDocument` contains a visibility flag and ordered elements.
- Element coordinates and widths are normalized to the output video, not platform pixels.
- Element kinds are pen, line, rectangle, ellipse, and arrow.
- `getAnnotationDocument` and `setAnnotationDocument` are the platform-neutral Tauri commands.
- `createAnnotationController` provides draft updates, object erasing, undo, redo, clear, and visibility control.

The controller is UI-agnostic. Products may render any toolbar or pointer layer while using the same document interface.

## Runtime design

Annotation composition belongs between capture and publishing so every publisher sees identical output. macOS already sends BGRA frames through this seam. On Windows, annotation-enabled sessions intentionally request the CPU BGRA capture path; sessions that do not opt in retain the existing D3D11 GPU surface path.

The pipeline retains the latest unannotated source frame. A document update queues and waits for a refreshed publication through the same latest-frame worker as normal capture, which keeps annotation latency independent from screen-content changes without allowing stale frames to overtake it. Documents are immutable shared snapshots.

## Validation and lifecycle

- Reject non-finite/out-of-range coordinates, invalid widths, duplicate or empty IDs, and oversized documents.
- Annotation documents are isolated per capture session and disappear with that session.
- Calling annotation commands for a session that did not opt in returns `invalidAnnotation`.
- Existing capture calls remain annotation-disabled by default.

## Acceptance criteria

1. The TypeScript and Rust contracts serialize to the same camelCase document model.
2. Pen and all supported shapes are composited before publisher delivery.
3. Updating a document republishes the latest frame even when the captured screen is static.
4. Two simultaneous sessions never share annotation state.
5. Annotation-disabled Windows sessions continue advertising GPU surface support.
6. Full Rust tests and TypeScript build pass.
