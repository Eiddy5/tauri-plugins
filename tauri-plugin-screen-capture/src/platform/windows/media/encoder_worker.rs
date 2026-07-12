use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use crate::{
    models::CaptureStats,
    pipeline::frame::VideoFrame,
    webrtc::{
        h264_encoder::H264Encoder,
        track::{EncodedVideoSample, WebRtcH264SampleSender},
    },
    Result,
};

pub(crate) struct WindowsEncoderWorker {
    frame_slot: Arc<LatestSlot<VideoFrame>>,
    sample_slot: Arc<LatestSlot<EncodedVideoSample>>,
    force_keyframe: Arc<AtomicBool>,
    telemetry: Arc<EncoderTelemetry>,
    encoder_thread: Mutex<Option<JoinHandle<()>>>,
    transport_thread: Mutex<Option<JoinHandle<()>>>,
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
        let sample_slot = Arc::new(LatestSlot::default());
        let force_keyframe = Arc::new(AtomicBool::new(false));
        let telemetry = Arc::new(EncoderTelemetry::new());

        let transport_slot = Arc::clone(&sample_slot);
        let transport_telemetry = Arc::clone(&telemetry);
        let transport_thread = thread::Builder::new()
            .name("screen-capture-h264-transport".to_string())
            .spawn(move || run_transport(transport_slot, transport_telemetry, sender, runtime))
            .map_err(worker_start_error)?;

        let (init_sender, init_receiver) = mpsc::sync_channel(1);
        let worker_frames = Arc::clone(&frame_slot);
        let worker_samples = Arc::clone(&sample_slot);
        let worker_keyframe = Arc::clone(&force_keyframe);
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
                    worker_frames,
                    worker_samples,
                    worker_keyframe,
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
            telemetry,
            encoder_thread: Mutex::new(Some(encoder_thread)),
            transport_thread: Mutex::new(Some(transport_thread)),
        })
    }

    pub(crate) fn submit(&self, frame: VideoFrame) {
        if self.frame_slot.replace(frame) {
            self.telemetry.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn request_keyframe(&self) {
        self.force_keyframe.store(true, Ordering::Release);
    }

    pub(crate) fn stats(&self) -> CaptureStats {
        self.telemetry.snapshot()
    }

    pub(crate) fn stop(&self) {
        self.frame_slot.stop();
        join_worker(&self.encoder_thread, "encoder");
        self.sample_slot.stop();
        join_worker(&self.transport_thread, "transport");
    }
}

impl Drop for WindowsEncoderWorker {
    fn drop(&mut self) {
        self.frame_slot.stop();
        if let Some(thread) = self
            .encoder_thread
            .get_mut()
            .expect("encoder thread lock")
            .take()
        {
            let _ = thread.join();
        }
        self.sample_slot.stop();
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

struct EncoderTelemetry {
    published: AtomicU64,
    dropped: AtomicU64,
    published_at: Mutex<VecDeque<Instant>>,
    backend: Mutex<String>,
}

impl EncoderTelemetry {
    fn new() -> Self {
        Self {
            published: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
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
    frame_slot: Arc<LatestSlot<VideoFrame>>,
    sample_slot: Arc<LatestSlot<EncodedVideoSample>>,
    force_keyframe: Arc<AtomicBool>,
    telemetry: Arc<EncoderTelemetry>,
) {
    let mut backend = encoder.backend_name();
    while let Some(frame) = frame_slot.wait_next() {
        if force_keyframe.swap(false, Ordering::AcqRel) && encoder.force_keyframe().is_err() {
            telemetry.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let result = encoder.encode_frame(&frame);
        let active_backend = encoder.backend_name();
        if active_backend != backend {
            backend = active_backend;
            telemetry.set_backend(active_backend.to_string());
        }
        match result {
            Ok(Some(sample)) => {
                if sample_slot.replace(sample) {
                    telemetry.dropped.fetch_add(1, Ordering::Relaxed);
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

fn run_transport(
    sample_slot: Arc<LatestSlot<EncodedVideoSample>>,
    telemetry: Arc<EncoderTelemetry>,
    sender: WebRtcH264SampleSender,
    runtime: tokio::runtime::Handle,
) {
    while let Some(sample) = sample_slot.wait_next() {
        match runtime.block_on(sender.send_sample(sample)) {
            Ok(()) => telemetry.record_published(),
            Err(error) => {
                telemetry.dropped.fetch_add(1, Ordering::Relaxed);
                eprintln!("[screen-capture] Windows encoded sample publish failed: {error}");
            }
        }
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
    use super::LatestSlot;
    use crate::{pipeline::frame::VideoFrame, PixelFormat};
    use std::sync::Arc;

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
        assert!(!slot.replace(frame(1)));
        assert!(slot.replace(frame(2)));
        assert_eq!(slot.wait_next().expect("latest frame").timestamp_ns, 2);
    }
}
