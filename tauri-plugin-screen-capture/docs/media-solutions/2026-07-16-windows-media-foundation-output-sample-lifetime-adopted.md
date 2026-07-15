# Windows Media Foundation output sample lifetime

## Status

Adopted on 2026-07-16.

## Problem

Long-running Windows screen sharing continuously increased process private memory. A 45-second
WebRTC loopback capture increased private memory by about 3.35 GiB while WebView2 child processes,
thread count, and handle count remained stable. The same growth reproduced with `H264Encoder`
alone, which excluded Windows Graphics Capture, WebRTC, and the WebView from the minimal case.

`MediaFoundationH264Encoder::take_output` allocated a caller-owned `IMFSample` containing at least
one MiB for every encoded frame. The active hardware encoder reported output stream flags `0x103`,
including `MFT_OUTPUT_STREAM_PROVIDES_SAMPLES`, and `cbSize=0`. For that allocation model the MFT
must provide the output sample and the caller must pass a null `pSample`. Supplying a caller sample
allowed the transform to replace the raw output pointer without releasing the original Rust-owned
sample and buffer.

## Decision

Select the output allocation model from `MFT_OUTPUT_STREAM_INFO` on every `ProcessOutput` call:

- `MFT_OUTPUT_STREAM_PROVIDES_SAMPLES`: pass a null `pSample`.
- `MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES`: prefer a null `pSample` and let the MFT allocate.
- Neither flag: allocate a caller sample using the exact reported `cbSize` and `cbAlignment`.
- Reject caller-allocated streams that report `cbSize=0` instead of inventing an arbitrary buffer
  size.

This changes only sample ownership. Capture resolution, NV12 conversion, bitrate, frame rate,
profile, keyframe behavior, and H.264 normalization remain unchanged.

## Verification

The ignored Windows hardware tests provide the acceptance loop:

```powershell
cargo test --test windows_encoder_performance -- --ignored --nocapture --test-threads=1
cargo test --test windows_capture_performance display_capture_encodes_gpu_surfaces_with_media_foundation -- --ignored --nocapture --exact --test-threads=1
```

On the diagnosing machine:

- The new memory regression encoded 360/360 measured frames after warmup with `0 MiB` private
  memory growth.
- The 1080p60 encoder benchmark sustained `65.42 FPS`.
- Forced keyframe output still contained an IDR access unit.
- The D3D11 surface path encoded at `52.74 FPS` with zero CPU readback.
