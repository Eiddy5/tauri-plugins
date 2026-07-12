use std::{
    collections::VecDeque,
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    models::CaptureStats,
    pipeline::frame::VideoFrame,
    platform::windows::media::WindowsGpuSurface,
    webrtc::{
        h264_encoder::H264Encoder,
        track::{EncodedVideoSample, WebRtcH264SampleSender},
    },
    Result,
};

pub(crate) struct WindowsEncoderWorker {
    frame_slot: Arc<LatestSlot<EncoderInput>>,
    sample_slot: Arc<OrderedSlot<EncodedVideoSample>>,
    force_keyframe: Arc<AtomicBool>,
    target_bitrate_bps: Arc<AtomicU64>,
    maximum_bitrate_bps: u64,
    telemetry: Arc<EncoderTelemetry>,
    encoder_thread: Mutex<Option<JoinHandle<()>>>,
    transport_thread: Mutex<Option<JoinHandle<()>>>,
}

enum EncoderInput {
    Cpu(VideoFrame),
    Gpu(WindowsGpuSurface),
}

impl WindowsEncoderWorker {
    pub(crate) fn start(
        width: u32,
        height: u32,
        fps: u32,
        sender: WebRtcH264SampleSender,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self> {
        let frame_slot = Arc::new(LatestSlot::default());
        let sample_slot = Arc::new(OrderedSlot::default());
        let force_keyframe = Arc::new(AtomicBool::new(false));
        let target_bitrate_bps = Arc::new(AtomicU64::new(0));
        let maximum_bitrate_bps = recommended_bitrate(width, height, fps);
        let telemetry = Arc::new(EncoderTelemetry::new());

        let transport_slot = Arc::clone(&sample_slot);
        let transport_keyframe = Arc::clone(&force_keyframe);
        let transport_telemetry = Arc::clone(&telemetry);
        let transport_thread = thread::Builder::new()
            .name("screen-capture-h264-transport".to_string())
            .spawn(move || {
                run_transport(
                    transport_slot,
                    transport_keyframe,
                    transport_telemetry,
                    sender,
                    runtime,
                )
            })
            .map_err(worker_start_error)?;

        let (init_sender, init_receiver) = mpsc::sync_channel(1);
        let worker_frames = Arc::clone(&frame_slot);
        let worker_samples = Arc::clone(&sample_slot);
        let worker_keyframe = Arc::clone(&force_keyframe);
        let worker_bitrate = Arc::clone(&target_bitrate_bps);
        let worker_telemetry = Arc::clone(&telemetry);
        let encoder_thread = match thread::Builder::new()
            .name("screen-capture-h264-encoder".to_string())
            .spawn(move || {
                let encoder = match H264Encoder::new(width, height, fps) {
                    Ok(encoder) => encoder,
                    Err(error) => {
                        let _ = init_sender.send(Err(error));
                        return;
                    }
                };
                let backend = encoder.backend_name().to_string();
                worker_telemetry.set_backend(backend.clone());
                if init_sender.send(Ok(backend)).is_err() {
                    return;
                }
                run_encoder(
                    encoder,
                    fps,
                    worker_frames,
                    worker_samples,
                    worker_keyframe,
                    worker_bitrate,
                    worker_telemetry,
                );
            }) {
            Ok(thread) => thread,
            Err(error) => {
                frame_slot.stop();
                sample_slot.stop();
                let _ = transport_thread.join();
                return Err(worker_start_error(error));
            }
        };

        match init_receiver.recv() {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                frame_slot.stop();
                sample_slot.stop();
                let _ = encoder_thread.join();
                let _ = transport_thread.join();
                return Err(error);
            }
            Err(error) => {
                frame_slot.stop();
                sample_slot.stop();
                let _ = encoder_thread.join();
                let _ = transport_thread.join();
                return Err(crate::Error::new(
                    crate::CaptureErrorCode::WebRtcTrackFailed,
                    format!("Windows H264 worker initialization stopped: {error}"),
                    true,
                ));
            }
        }

        Ok(Self {
            frame_slot,
            sample_slot,
            force_keyframe,
            target_bitrate_bps,
            maximum_bitrate_bps,
            telemetry,
            encoder_thread: Mutex::new(Some(encoder_thread)),
            transport_thread: Mutex::new(Some(transport_thread)),
        })
    }

    pub(crate) fn submit(&self, frame: VideoFrame) {
        if self.frame_slot.replace(EncoderInput::Cpu(frame)) {
            self.telemetry.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn submit_gpu(&self, surface: WindowsGpuSurface) {
        if self.frame_slot.replace(EncoderInput::Gpu(surface)) {
            self.telemetry.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn request_keyframe(&self) {
        self.force_keyframe.store(true, Ordering::Release);
    }

    pub(crate) fn update_estimated_bitrate(&self, estimated_bitrate_bps: Option<u64>) {
        self.target_bitrate_bps.store(
            target_bitrate_from_remb(self.maximum_bitrate_bps, estimated_bitrate_bps),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn stats(&self) -> CaptureStats {
        self.telemetry.snapshot()
    }

    pub(crate) fn stop(&self) {
        self.frame_slot.stop();
        self.sample_slot.stop();
        join_worker(&self.encoder_thread, "encoder");
        join_worker(&self.transport_thread, "transport");
    }
}

impl Drop for WindowsEncoderWorker {
    fn drop(&mut self) {
        self.frame_slot.stop();
        self.sample_slot.stop();
        if let Some(thread) = self
            .encoder_thread
            .get_mut()
            .expect("encoder thread lock")
            .take()
        {
            let _ = thread.join();
        }
        if let Some(thread) = self
            .transport_thread
            .get_mut()
            .expect("transport thread lock")
            .take()
        {
            let _ = thread.join();
        }
    }
}

struct LatestSlot<T> {
    state: Mutex<LatestState<T>>,
    changed: Condvar,
}

struct LatestState<T> {
    value: Option<T>,
    stopped: bool,
}

impl<T> Default for LatestSlot<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(LatestState {
                value: None,
                stopped: false,
            }),
            changed: Condvar::new(),
        }
    }
}

impl<T> LatestSlot<T> {
    fn replace(&self, value: T) -> bool {
        let mut state = self.state.lock().expect("latest slot lock");
        if state.stopped {
            return true;
        }
        let replaced = state.value.replace(value).is_some();
        self.changed.notify_one();
        replaced
    }

    fn wait_next(&self) -> Option<T> {
        let mut state = self.state.lock().expect("latest slot lock");
        while state.value.is_none() && !state.stopped {
            state = self.changed.wait(state).expect("latest slot wait");
        }
        state.value.take()
    }

    fn stop(&self) {
        let mut state = self.state.lock().expect("latest slot lock");
        state.stopped = true;
        state.value = None;
        self.changed.notify_all();
    }
}

struct OrderedSlot<T> {
    state: Mutex<LatestState<T>>,
    changed: Condvar,
}

impl<T> Default for OrderedSlot<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(LatestState {
                value: None,
                stopped: false,
            }),
            changed: Condvar::new(),
        }
    }
}

impl<T> OrderedSlot<T> {
    fn send(&self, value: T) -> bool {
        let mut state = self.state.lock().expect("ordered slot lock");
        while state.value.is_some() && !state.stopped {
            state = self.changed.wait(state).expect("ordered slot send wait");
        }
        if state.stopped {
            return false;
        }
        state.value = Some(value);
        self.changed.notify_one();
        true
    }

    fn recv(&self) -> Option<T> {
        let mut state = self.state.lock().expect("ordered slot lock");
        while state.value.is_none() && !state.stopped {
            state = self.changed.wait(state).expect("ordered slot receive wait");
        }
        let value = state.value.take();
        self.changed.notify_all();
        value
    }

    fn stop(&self) {
        let mut state = self.state.lock().expect("ordered slot lock");
        state.stopped = true;
        state.value = None;
        self.changed.notify_all();
    }
}

struct EncoderTelemetry {
    published: AtomicU64,
    dropped: AtomicU64,
    cpu_readback: AtomicU64,
    bitrate_bps: AtomicU64,
    published_at: Mutex<VecDeque<Instant>>,
    backend: Mutex<String>,
}

impl EncoderTelemetry {
    fn new() -> Self {
        Self {
            published: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            cpu_readback: AtomicU64::new(0),
            bitrate_bps: AtomicU64::new(0),
            published_at: Mutex::new(VecDeque::new()),
            backend: Mutex::new("initializing".to_string()),
        }
    }

    fn set_backend(&self, backend: String) {
        *self.backend.lock().expect("encoder backend lock") = backend;
    }

    fn record_published(&self) {
        self.published.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        let mut samples = self.published_at.lock().expect("published samples lock");
        samples.push_back(now);
        trim_fps_window(&mut samples, now);
    }

    fn snapshot(&self) -> CaptureStats {
        let now = Instant::now();
        let mut samples = self.published_at.lock().expect("published samples lock");
        trim_fps_window(&mut samples, now);
        let dropped = self.dropped.load(Ordering::Relaxed);
        let publish_fps = samples.len() as f64;
        CaptureStats {
            frames_published: self.published.load(Ordering::Relaxed),
            frames_dropped: dropped,
            frames_encoder_dropped: dropped,
            frames_cpu_readback: self.cpu_readback.load(Ordering::Relaxed),
            bitrate_kbps: self.bitrate_bps.load(Ordering::Relaxed).div_ceil(1_000) as u32,
            fps: publish_fps,
            publish_fps,
            encoder_backend: Some(self.backend.lock().expect("encoder backend lock").clone()),
            started: true,
            ..CaptureStats::default()
        }
    }
}

fn run_encoder(
    mut encoder: H264Encoder,
    fps: u32,
    frame_slot: Arc<LatestSlot<EncoderInput>>,
    sample_slot: Arc<OrderedSlot<EncodedVideoSample>>,
    force_keyframe: Arc<AtomicBool>,
    target_bitrate_bps: Arc<AtomicU64>,
    telemetry: Arc<EncoderTelemetry>,
) {
    let mut backend = encoder.backend_name();
    let mut gpu_failed = false;
    let mut applied_bitrate_bps = 0;
    let mut rejected_bitrate_bps = None;
    while let Some(frame) = frame_slot.wait_next() {
        let requested_bitrate_bps = target_bitrate_bps.load(Ordering::Relaxed);
        if requested_bitrate_bps > 0
            && bitrate_change_is_material(applied_bitrate_bps, requested_bitrate_bps)
            && rejected_bitrate_bps != Some(requested_bitrate_bps)
        {
            match encoder.set_bitrate_bps(requested_bitrate_bps as u32) {
                Ok(()) => {
                    applied_bitrate_bps = requested_bitrate_bps;
                    rejected_bitrate_bps = None;
                    telemetry
                        .bitrate_bps
                        .store(requested_bitrate_bps, Ordering::Relaxed);
                }
                Err(error) => {
                    rejected_bitrate_bps = Some(requested_bitrate_bps);
                    telemetry.bitrate_bps.store(0, Ordering::Relaxed);
                    eprintln!(
                        "[screen-capture] Windows encoder rejected dynamic bitrate {requested_bitrate_bps} bps: {error}"
                    );
                }
            }
        }
        if matches!(frame, EncoderInput::Cpu(_))
            && encoder.backend_name() == "media-foundation-d3d11-surface"
        {
            match H264Encoder::new(encoder.width(), encoder.height(), fps) {
                Ok(cpu_encoder) => {
                    encoder = cpu_encoder;
                    applied_bitrate_bps = 0;
                    rejected_bitrate_bps = None;
                }
                Err(error) => {
                    telemetry.dropped.fetch_add(1, Ordering::Relaxed);
                    eprintln!("[screen-capture] Windows CPU fallback encoder failed: {error}");
                    continue;
                }
            }
        }
        if force_keyframe.swap(false, Ordering::AcqRel) && encoder.force_keyframe().is_err() {
            telemetry.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let result = match &frame {
            EncoderInput::Cpu(frame) => encoder.encode_frame(frame),
            EncoderInput::Gpu(surface) => {
                encode_gpu_with_fallback(&mut encoder, surface, fps, &telemetry, &mut gpu_failed)
            }
        };
        let active_backend = encoder.backend_name();
        if active_backend != backend {
            backend = active_backend;
            applied_bitrate_bps = 0;
            rejected_bitrate_bps = None;
            telemetry.set_backend(active_backend.to_string());
        }
        match result {
            Ok(Some(sample)) => {
                if !sample_slot.send(sample) {
                    return;
                }
            }
            Ok(None) => {
                telemetry.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                telemetry.dropped.fetch_add(1, Ordering::Relaxed);
                eprintln!("[screen-capture] Windows H264 encode failed: {error}");
            }
        }
    }
}

fn recommended_bitrate(width: u32, height: u32, fps: u32) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(u64::from(fps.max(1)))
        .saturating_mul(8)
        .div_ceil(100)
        .clamp(4_000_000, 8_000_000)
}

fn target_bitrate_from_remb(maximum_bitrate_bps: u64, estimated_bitrate_bps: Option<u64>) -> u64 {
    let Some(estimated_bitrate_bps) = estimated_bitrate_bps else {
        return maximum_bitrate_bps;
    };
    let minimum_bitrate_bps = (maximum_bitrate_bps / 8).max(300_000);
    estimated_bitrate_bps
        .saturating_mul(85)
        .div_ceil(100)
        .clamp(minimum_bitrate_bps, maximum_bitrate_bps)
}

fn bitrate_change_is_material(current: u64, requested: u64) -> bool {
    current == 0 || current.abs_diff(requested) >= current / 10
}

fn encode_gpu_with_fallback(
    encoder: &mut H264Encoder,
    surface: &WindowsGpuSurface,
    fps: u32,
    telemetry: &EncoderTelemetry,
    gpu_failed: &mut bool,
) -> Result<Option<EncodedVideoSample>> {
    if !*gpu_failed && encoder.backend_name() != "media-foundation-d3d11-surface" {
        match H264Encoder::new_gpu_surface(surface, fps) {
            Ok(gpu_encoder) => *encoder = gpu_encoder,
            Err(error) => {
                *gpu_failed = true;
                eprintln!(
                    "[screen-capture] Windows GPU surface encoder unavailable; using readback fallback: {error}"
                );
            }
        }
    }

    if encoder.backend_name() == "media-foundation-d3d11-surface" {
        match encoder.encode_gpu_surface(surface) {
            Ok(sample) => return Ok(sample),
            Err(error) => {
                *gpu_failed = true;
                eprintln!(
                    "[screen-capture] Windows GPU surface encode failed; using readback fallback: {error}"
                );
                *encoder = H264Encoder::new(surface.width(), surface.height(), fps)?;
            }
        }
    }

    let nv12 = surface.readback_nv12()?;
    telemetry.cpu_readback.fetch_add(1, Ordering::Relaxed);
    encoder.encode_nv12_readback(&nv12, surface.timestamp_ns())
}

fn run_transport(
    sample_slot: Arc<OrderedSlot<EncodedVideoSample>>,
    force_keyframe: Arc<AtomicBool>,
    telemetry: Arc<EncoderTelemetry>,
    sender: WebRtcH264SampleSender,
    runtime: tokio::runtime::Handle,
) {
    const SEND_TIMEOUT: Duration = Duration::from_secs(2);
    let mut waiting_for_idr = false;
    while let Some(sample) = sample_slot.recv() {
        if should_drop_until_idr(waiting_for_idr, &sample, &force_keyframe) {
            telemetry.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let is_idr = sample.is_h264_idr();
        match block_on_transport_timeout(&runtime, SEND_TIMEOUT, sender.send_sample(sample)) {
            Ok(Ok(())) => {
                if is_idr {
                    waiting_for_idr = false;
                }
                telemetry.record_published();
            }
            Ok(Err(error)) => {
                waiting_for_idr = true;
                force_keyframe.store(true, Ordering::Release);
                telemetry.dropped.fetch_add(1, Ordering::Relaxed);
                eprintln!("[screen-capture] Windows encoded sample publish failed: {error}");
            }
            Err(_) => {
                waiting_for_idr = true;
                force_keyframe.store(true, Ordering::Release);
                telemetry.dropped.fetch_add(1, Ordering::Relaxed);
                eprintln!(
                    "[screen-capture] Windows encoded sample publish timed out after {} ms; requesting IDR",
                    SEND_TIMEOUT.as_millis()
                );
            }
        }
    }
}

fn block_on_transport_timeout<F, T>(
    runtime: &tokio::runtime::Handle,
    timeout: Duration,
    future: F,
) -> std::result::Result<T, tokio::time::error::Elapsed>
where
    F: Future<Output = T>,
{
    runtime.block_on(async move { tokio::time::timeout(timeout, future).await })
}

fn should_drop_until_idr(
    waiting_for_idr: bool,
    sample: &EncodedVideoSample,
    force_keyframe: &AtomicBool,
) -> bool {
    if waiting_for_idr && !sample.is_h264_idr() {
        force_keyframe.store(true, Ordering::Release);
        true
    } else {
        false
    }
}

fn trim_fps_window(samples: &mut VecDeque<Instant>, now: Instant) {
    while samples
        .front()
        .is_some_and(|timestamp| now.duration_since(*timestamp).as_secs_f64() > 1.0)
    {
        samples.pop_front();
    }
}

fn join_worker(worker: &Mutex<Option<JoinHandle<()>>>, name: &str) {
    if let Some(thread) = worker.lock().expect("worker thread lock").take() {
        if thread.join().is_err() {
            eprintln!("[screen-capture] Windows {name} worker panicked while stopping");
        }
    }
}

fn worker_start_error(error: impl std::fmt::Display) -> crate::Error {
    crate::Error::new(
        crate::CaptureErrorCode::WebRtcTrackFailed,
        format!("failed to start Windows H264 worker: {error}"),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        block_on_transport_timeout, should_drop_until_idr, EncoderInput, LatestSlot, OrderedSlot,
    };
    use crate::webrtc::track::EncodedVideoSample;
    use crate::{pipeline::frame::VideoFrame, PixelFormat};
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        sync::Arc,
        thread,
        time::Duration,
    };

    fn frame(timestamp_ns: u64) -> VideoFrame {
        VideoFrame {
            width: 2,
            height: 2,
            pixel_format: PixelFormat::Bgra,
            timestamp_ns,
            data: Arc::from([0; 16]),
        }
    }

    #[test]
    fn latest_frame_slot_replaces_stale_frame() {
        let slot = LatestSlot::default();
        assert!(!slot.replace(EncoderInput::Cpu(frame(1))));
        assert!(slot.replace(EncoderInput::Cpu(frame(2))));
        let EncoderInput::Cpu(frame) = slot.wait_next().expect("latest frame") else {
            panic!("expected CPU frame");
        };
        assert_eq!(frame.timestamp_ns, 2);
    }

    #[test]
    fn encoded_sample_handoff_preserves_order_under_backpressure() {
        let slot = Arc::new(OrderedSlot::default());
        assert!(slot.send(1));

        let producer_slot = Arc::clone(&slot);
        let producer = thread::spawn(move || producer_slot.send(2));
        thread::sleep(Duration::from_millis(20));
        assert!(
            !producer.is_finished(),
            "second sample must wait, not replace"
        );

        assert_eq!(slot.recv(), Some(1));
        assert!(producer.join().expect("ordered producer"));
        assert_eq!(slot.recv(), Some(2));
    }

    #[test]
    fn remb_target_bitrate_keeps_transport_headroom() {
        assert_eq!(super::target_bitrate_from_remb(8_000_000, None), 8_000_000);
        assert_eq!(
            super::target_bitrate_from_remb(8_000_000, Some(0)),
            1_000_000
        );
        assert_eq!(
            super::target_bitrate_from_remb(8_000_000, Some(4_800_000)),
            4_080_000
        );
        assert_eq!(
            super::target_bitrate_from_remb(8_000_000, Some(20_000_000)),
            8_000_000
        );
        assert_eq!(
            super::target_bitrate_from_remb(8_000_000, Some(138_000)),
            1_000_000
        );
    }

    #[test]
    fn recovery_repeats_keyframe_request_until_idr_arrives() {
        let force_keyframe = AtomicBool::new(false);
        let p_frame = EncodedVideoSample {
            data: vec![0, 0, 0, 1, 0x41, 1],
            duration: Duration::from_millis(16),
            timestamp_ns: 1,
        };
        assert!(should_drop_until_idr(true, &p_frame, &force_keyframe));
        assert!(force_keyframe.load(Ordering::Acquire));

        let idr = EncodedVideoSample {
            data: vec![0, 0, 0, 1, 0x65, 1],
            duration: Duration::from_millis(16),
            timestamp_ns: 2,
        };
        assert!(!should_drop_until_idr(true, &idr, &force_keyframe));
    }

    #[test]
    fn transport_timeout_enters_runtime_before_constructing_timer() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime");
        let handle = runtime.handle().clone();
        let result = thread::spawn(move || {
            block_on_transport_timeout(&handle, Duration::from_millis(50), async { 7 })
        })
        .join();

        assert_eq!(result.expect("transport thread must not panic"), Ok(7));
    }
}
