#![cfg(windows)]

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use tauri_plugin_screen_capture::platform::windows::media::WindowsGpuSurface;
use tauri_plugin_screen_capture::{
    capture::{CaptureBackend, FrameConsumer, WindowsCaptureBackend},
    pipeline::frame::VideoFrame,
    webrtc::h264_encoder::H264Encoder,
    CaptureSourceKind, ListSourcesOptions, Result, StartCaptureOptions,
};

#[derive(Clone, Default)]
struct FrameProbe {
    arrivals: Arc<Mutex<Vec<Instant>>>,
    sizes: Arc<Mutex<Vec<(u32, u32)>>>,
}

struct EncodingProbe {
    encoder: Mutex<H264Encoder>,
    encoded: AtomicUsize,
}

#[derive(Default)]
struct GpuSurfaceProbe {
    gpu_frames: AtomicUsize,
    cpu_frames: AtomicUsize,
    sizes: Mutex<Vec<(u32, u32)>>,
}

struct GpuEncodingProbe {
    sender: Mutex<Option<mpsc::SyncSender<WindowsGpuSurface>>>,
    encoded: Arc<AtomicUsize>,
    backend: Arc<Mutex<Option<String>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl GpuEncodingProbe {
    fn new(fps: u32) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<WindowsGpuSurface>(1);
        let encoded = Arc::new(AtomicUsize::new(0));
        let backend = Arc::new(Mutex::new(None));
        let worker_encoded = Arc::clone(&encoded);
        let worker_backend = Arc::clone(&backend);
        let worker = thread::Builder::new()
            .name("gpu-surface-test-encoder".to_string())
            .spawn(move || {
                let mut encoder = None;
                while let Ok(surface) = receiver.recv() {
                    if encoder.is_none() {
                        let created = H264Encoder::new_gpu_surface(&surface, fps)
                            .expect("create D3D11 surface H264 encoder");
                        *worker_backend.lock().unwrap() = Some(created.backend_name().to_string());
                        encoder = Some(created);
                    }
                    if encoder
                        .as_mut()
                        .unwrap()
                        .encode_gpu_surface(&surface)
                        .expect("encode D3D11 NV12 surface")
                        .is_some()
                    {
                        worker_encoded.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
            .expect("start GPU encoder test thread");
        Self {
            sender: Mutex::new(Some(sender)),
            encoded,
            backend,
            worker: Mutex::new(Some(worker)),
        }
    }
}

impl Drop for GpuEncodingProbe {
    fn drop(&mut self) {
        self.sender.get_mut().unwrap().take();
        if let Some(worker) = self.worker.get_mut().unwrap().take() {
            let _ = worker.join();
        }
    }
}

#[async_trait]
impl FrameConsumer for GpuEncodingProbe {
    fn supports_gpu_surfaces(&self) -> bool {
        true
    }

    async fn push_frame(&self, _frame: VideoFrame) -> Result<()> {
        panic!("GPU encoder probe must not receive CPU frames")
    }

    async fn push_gpu_surface(&self, surface: WindowsGpuSurface) -> Result<()> {
        if let Some(sender) = self.sender.lock().unwrap().as_ref() {
            let _ = sender.try_send(surface);
        }
        Ok(())
    }
}

#[async_trait]
impl FrameConsumer for GpuSurfaceProbe {
    fn supports_gpu_surfaces(&self) -> bool {
        true
    }

    async fn push_frame(&self, _frame: VideoFrame) -> Result<()> {
        self.cpu_frames.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn push_gpu_surface(&self, surface: WindowsGpuSurface) -> Result<()> {
        self.gpu_frames.fetch_add(1, Ordering::Relaxed);
        self.sizes
            .lock()
            .unwrap()
            .push((surface.width(), surface.height()));
        Ok(())
    }
}

#[async_trait]
impl FrameConsumer for EncodingProbe {
    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        if self.encoder.lock().unwrap().encode_frame(&frame)?.is_some() {
            self.encoded.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

#[async_trait]
impl FrameConsumer for FrameProbe {
    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        self.arrivals.lock().unwrap().push(Instant::now());
        self.sizes.lock().unwrap().push((frame.width, frame.height));
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an interactive Windows desktop"]
async fn display_capture_sustains_smooth_1080p60_delivery() {
    let backend = WindowsCaptureBackend;
    let source = backend
        .list_sources(ListSourcesOptions {
            kinds: vec![CaptureSourceKind::Display],
            include_thumbnails: false,
            include_current_app: true,
            include_system_ui: true,
            debug_raw_sources: false,
        })
        .await
        .expect("list Windows displays")
        .into_iter()
        .find(|source| source.kind == CaptureSourceKind::Display)
        .expect("at least one Windows display");

    let use_native_size = std::env::var_os("SCREEN_CAPTURE_BENCH_NATIVE").is_some();
    let target_width = if use_native_size {
        source.width
    } else {
        source.width.min(1920)
    }
    .max(2)
        & !1;
    let scale = target_width as f64 / source.width.max(1) as f64;
    let target_height = ((source.height as f64 * scale).round() as u32).max(2) & !1;
    let probe = FrameProbe::default();
    let running = backend
        .start_capture(
            StartCaptureOptions {
                source_id: source.id,
                source_kind: CaptureSourceKind::Display,
                fps: Some(60),
                width: Some(target_width),
                height: Some(target_height),
                capture_cursor: Some(true),
                publisher: None,
                annotations: None,
            },
            Box::new(probe.clone()),
        )
        .await
        .expect("start Windows display capture");

    tokio::time::sleep(Duration::from_millis(750)).await;
    probe.arrivals.lock().unwrap().clear();
    probe.sizes.lock().unwrap().clear();
    let measurement_started = Instant::now();
    tokio::time::sleep(Duration::from_secs(4)).await;
    let elapsed = measurement_started.elapsed();
    running.stop().await.expect("stop Windows display capture");

    let arrivals = probe.arrivals.lock().unwrap().clone();
    let sizes = probe.sizes.lock().unwrap().clone();
    let delivered_fps = arrivals.len() as f64 / elapsed.as_secs_f64();
    let mut frame_gaps = arrivals
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]))
        .collect::<Vec<_>>();
    frame_gaps.sort_unstable();
    let p95_gap = frame_gaps
        .get(frame_gaps.len().saturating_mul(95) / 100)
        .copied()
        .unwrap_or(elapsed);

    eprintln!(
        "WINDOWS_CAPTURE_BASELINE source={}x{} output={}x{} frames={} fps={:.2} p95_gap_ms={:.2}",
        source.width,
        source.height,
        target_width,
        target_height,
        arrivals.len(),
        delivered_fps,
        p95_gap.as_secs_f64() * 1000.0,
    );

    assert!(
        sizes
            .iter()
            .all(|size| *size == (target_width, target_height)),
        "capture must preserve the requested output dimensions"
    );
    assert!(
        delivered_fps >= 50.0,
        "expected at least 50 delivered FPS for smooth 1080p60 capture, got {delivered_fps:.2}"
    );
    assert!(
        p95_gap <= Duration::from_millis(34),
        "expected p95 frame gap <= 34 ms, got {:.2} ms",
        p95_gap.as_secs_f64() * 1000.0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an interactive Windows desktop"]
async fn display_capture_resize_and_h264_encode_stay_realtime_end_to_end() {
    let backend = WindowsCaptureBackend;
    let source = backend
        .list_sources(ListSourcesOptions {
            kinds: vec![CaptureSourceKind::Display],
            include_thumbnails: false,
            include_current_app: true,
            include_system_ui: true,
            debug_raw_sources: false,
        })
        .await
        .expect("list Windows displays")
        .into_iter()
        .find(|source| source.kind == CaptureSourceKind::Display)
        .expect("at least one Windows display");
    let target_width = source.width.clamp(2, 1920) & !1;
    let scale = target_width as f64 / source.width.max(1) as f64;
    let target_height = ((source.height as f64 * scale).round() as u32).max(2) & !1;
    let probe = Arc::new(EncodingProbe {
        encoder: Mutex::new(
            H264Encoder::new(target_width, target_height, 60).expect("create Windows H264 encoder"),
        ),
        encoded: AtomicUsize::new(0),
    });

    let running = backend
        .start_capture(
            StartCaptureOptions {
                source_id: source.id,
                source_kind: CaptureSourceKind::Display,
                fps: Some(60),
                width: Some(target_width),
                height: Some(target_height),
                capture_cursor: Some(true),
                publisher: None,
                annotations: None,
            },
            Box::new(ArcEncodingProbe(Arc::clone(&probe))),
        )
        .await
        .expect("start Windows end-to-end capture");

    tokio::time::sleep(Duration::from_secs(1)).await;
    probe.encoded.store(0, Ordering::Relaxed);
    let started = Instant::now();
    tokio::time::sleep(Duration::from_secs(4)).await;
    running
        .stop()
        .await
        .expect("stop Windows end-to-end capture");
    let elapsed = started.elapsed();
    let encoded = probe.encoded.load(Ordering::Relaxed);
    let encoded_fps = encoded as f64 / elapsed.as_secs_f64();
    eprintln!(
        "WINDOWS_END_TO_END_BASELINE output={}x{} encoded={} fps={:.2}",
        target_width, target_height, encoded, encoded_fps,
    );
    assert!(
        encoded_fps >= 50.0,
        "expected display capture → resize → H264 encode >= 50 FPS, got {encoded_fps:.2}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an interactive Windows desktop with D3D11 video processing"]
async fn display_capture_delivers_gpu_surfaces_without_cpu_readback() {
    let backend = WindowsCaptureBackend;
    let source = backend
        .list_sources(ListSourcesOptions {
            kinds: vec![CaptureSourceKind::Display],
            include_thumbnails: false,
            include_current_app: true,
            include_system_ui: true,
            debug_raw_sources: false,
        })
        .await
        .expect("list Windows displays")
        .into_iter()
        .find(|source| source.kind == CaptureSourceKind::Display)
        .expect("at least one Windows display");
    let target_width = source.width.clamp(2, 1920) & !1;
    let scale = target_width as f64 / source.width.max(1) as f64;
    let target_height = ((source.height as f64 * scale).round() as u32).max(2) & !1;
    let probe = Arc::new(GpuSurfaceProbe::default());
    let running = backend
        .start_capture(
            StartCaptureOptions {
                source_id: source.id,
                source_kind: CaptureSourceKind::Display,
                fps: Some(60),
                width: Some(target_width),
                height: Some(target_height),
                capture_cursor: Some(true),
                publisher: None,
                annotations: None,
            },
            Box::new(ArcGpuSurfaceProbe(Arc::clone(&probe))),
        )
        .await
        .expect("start GPU surface capture");

    tokio::time::sleep(Duration::from_secs(3)).await;
    let stats = running.stats().await.expect("capture stats");
    running.stop().await.expect("stop GPU surface capture");

    assert!(
        probe.gpu_frames.load(Ordering::Relaxed) > 0,
        "expected D3D11 NV12 surfaces"
    );
    assert_eq!(probe.cpu_frames.load(Ordering::Relaxed), 0);
    assert_eq!(stats.frames_cpu_readback, 0);
    assert!(probe
        .sizes
        .lock()
        .unwrap()
        .iter()
        .all(|size| *size == (target_width, target_height)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an interactive Windows desktop and a D3D11-aware hardware H264 MFT"]
async fn display_capture_encodes_gpu_surfaces_with_media_foundation() {
    let backend = WindowsCaptureBackend;
    let source = backend
        .list_sources(ListSourcesOptions {
            kinds: vec![CaptureSourceKind::Display],
            include_thumbnails: false,
            include_current_app: true,
            include_system_ui: true,
            debug_raw_sources: false,
        })
        .await
        .expect("list Windows displays")
        .into_iter()
        .find(|source| source.kind == CaptureSourceKind::Display)
        .expect("at least one Windows display");
    let target_width = source.width.clamp(2, 1920) & !1;
    let scale = target_width as f64 / source.width.max(1) as f64;
    let target_height = ((source.height as f64 * scale).round() as u32).max(2) & !1;
    let probe = Arc::new(GpuEncodingProbe::new(60));
    let running = backend
        .start_capture(
            StartCaptureOptions {
                source_id: source.id,
                source_kind: CaptureSourceKind::Display,
                fps: Some(60),
                width: Some(target_width),
                height: Some(target_height),
                capture_cursor: Some(true),
                publisher: None,
                annotations: None,
            },
            Box::new(ArcGpuEncodingProbe(Arc::clone(&probe))),
        )
        .await
        .expect("start GPU encode capture");

    let started = Instant::now();
    tokio::time::sleep(Duration::from_secs(4)).await;
    let elapsed = started.elapsed();
    let stats = running.stats().await.expect("capture stats");
    running.stop().await.expect("stop GPU encode capture");

    let encoded = probe.encoded.load(Ordering::Relaxed);
    let encoded_fps = encoded as f64 / elapsed.as_secs_f64();
    eprintln!(
        "WINDOWS_GPU_SURFACE_ENCODE output={}x{} encoded={} fps={:.2} readback={}",
        target_width, target_height, encoded, encoded_fps, stats.frames_cpu_readback
    );
    assert!(encoded > 0);
    assert_eq!(stats.frames_cpu_readback, 0);
    assert_eq!(
        probe.backend.lock().unwrap().as_deref(),
        Some("media-foundation-d3d11-surface")
    );
}

struct ArcEncodingProbe(Arc<EncodingProbe>);

struct ArcGpuSurfaceProbe(Arc<GpuSurfaceProbe>);

struct ArcGpuEncodingProbe(Arc<GpuEncodingProbe>);

#[async_trait]
impl FrameConsumer for ArcGpuEncodingProbe {
    fn supports_gpu_surfaces(&self) -> bool {
        true
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        self.0.push_frame(frame).await
    }

    async fn push_gpu_surface(&self, surface: WindowsGpuSurface) -> Result<()> {
        self.0.push_gpu_surface(surface).await
    }
}

#[async_trait]
impl FrameConsumer for ArcGpuSurfaceProbe {
    fn supports_gpu_surfaces(&self) -> bool {
        true
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        self.0.push_frame(frame).await
    }

    async fn push_gpu_surface(&self, surface: WindowsGpuSurface) -> Result<()> {
        self.0.push_gpu_surface(surface).await
    }
}

#[async_trait]
impl FrameConsumer for ArcEncodingProbe {
    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        self.0.push_frame(frame).await
    }
}
