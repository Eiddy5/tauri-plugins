use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    models::CaptureStats,
    pipeline::frame::VideoFrame,
    platform::macos::media::MacGpuSurface,
    webrtc::{h264_encoder::H264Encoder, track::EncodedVideoSample},
    Result,
};

use super::track::WebRtcH264SampleSender;

const SAMPLE_QUEUE_CAPACITY: usize = 8;

#[derive(Default)]
pub struct EncoderControlState {
    force_keyframe: AtomicBool,
}

impl EncoderControlState {
    pub fn request_keyframe(&self) {
        self.force_keyframe.store(true, Ordering::Release);
    }

    pub fn take_force_keyframe(&self) -> bool {
        self.force_keyframe.swap(false, Ordering::AcqRel)
    }
}

pub struct OrderedSampleHandoff {
    capacity: usize,
    samples: Mutex<VecDeque<EncodedVideoSample>>,
}

impl OrderedSampleHandoff {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            samples: Mutex::new(VecDeque::with_capacity(capacity.max(1))),
        }
    }

    pub fn push(&self, sample: EncodedVideoSample) -> std::result::Result<(), EncodedVideoSample> {
        let mut samples = self.samples.lock().expect("sample handoff lock");
        if samples.len() >= self.capacity {
            return Err(sample);
        }
        samples.push_back(sample);
        Ok(())
    }

    pub fn pop(&self) -> Option<EncodedVideoSample> {
        self.samples.lock().ok()?.pop_front()
    }
}

enum EncoderInput {
    Cpu(VideoFrame),
    Gpu(MacGpuSurface),
}

#[derive(Default)]
struct PendingState {
    input: Option<EncoderInput>,
    stopping: bool,
}

#[derive(Default)]
struct WorkerTelemetry {
    frames_published: AtomicU64,
    frames_encoder_dropped: AtomicU64,
    frames_cpu_readback: AtomicU64,
}

pub struct MacEncoderWorker {
    pending: Arc<(Mutex<PendingState>, Condvar)>,
    control: Arc<EncoderControlState>,
    telemetry: Arc<WorkerTelemetry>,
    target_bitrate_bps: u32,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl MacEncoderWorker {
    pub fn start(
        width: u32,
        height: u32,
        fps: u32,
        sender: WebRtcH264SampleSender,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self> {
        let encoder = H264Encoder::new(width, height, fps)?;
        let target_bitrate_bps = encoder.target_bitrate_bps();
        let pending = Arc::new((Mutex::new(PendingState::default()), Condvar::new()));
        let control = Arc::new(EncoderControlState::default());
        let telemetry = Arc::new(WorkerTelemetry::default());
        let worker_pending = Arc::clone(&pending);
        let worker_control = Arc::clone(&control);
        let worker_telemetry = Arc::clone(&telemetry);
        let (sample_sender, mut sample_receiver) =
            tokio::sync::mpsc::channel::<EncodedVideoSample>(SAMPLE_QUEUE_CAPACITY);
        let publish_telemetry = Arc::clone(&telemetry);
        runtime.spawn(async move {
            while let Some(sample) = sample_receiver.recv().await {
                match sender.send_sample(sample).await {
                    Ok(()) => {
                        publish_telemetry
                            .frames_published
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) => {
                        publish_telemetry
                            .frames_encoder_dropped
                            .fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(%error, "failed to publish VideoToolbox sample");
                    }
                }
            }
        });
        let join = thread::Builder::new()
            .name("screen-capture-videotoolbox".to_string())
            .spawn(move || {
                run_worker(
                    encoder,
                    worker_pending,
                    worker_control,
                    worker_telemetry,
                    sample_sender,
                );
            })
            .map_err(|error| {
                crate::Error::new(
                    crate::CaptureErrorCode::WebRtcTrackFailed,
                    format!("failed to start VideoToolbox worker: {error}"),
                    true,
                )
            })?;

        Ok(Self {
            pending,
            control,
            telemetry,
            target_bitrate_bps,
            join: Mutex::new(Some(join)),
        })
    }

    pub fn submit_gpu(&self, surface: MacGpuSurface) {
        self.submit(EncoderInput::Gpu(surface));
    }

    pub fn submit_cpu(&self, frame: VideoFrame) {
        self.telemetry
            .frames_cpu_readback
            .fetch_add(1, Ordering::Relaxed);
        self.submit(EncoderInput::Cpu(frame));
    }

    fn submit(&self, input: EncoderInput) {
        let (state, ready) = &*self.pending;
        if let Ok(mut state) = state.lock() {
            if state.stopping {
                return;
            }
            if state.input.replace(input).is_some() {
                self.telemetry
                    .frames_encoder_dropped
                    .fetch_add(1, Ordering::Relaxed);
            }
            ready.notify_one();
        }
    }

    pub fn request_keyframe(&self) {
        self.control.request_keyframe();
        self.pending.1.notify_one();
    }

    pub const fn target_bitrate_bps(&self) -> u32 {
        self.target_bitrate_bps
    }

    pub fn stats(&self) -> CaptureStats {
        let frames_published = self.telemetry.frames_published.load(Ordering::Relaxed);
        let frames_encoder_dropped = self
            .telemetry
            .frames_encoder_dropped
            .load(Ordering::Relaxed);
        CaptureStats {
            frames_published,
            frames_dropped: frames_encoder_dropped,
            frames_encoder_dropped,
            frames_cpu_readback: self.telemetry.frames_cpu_readback.load(Ordering::Relaxed),
            bitrate_kbps: self.target_bitrate_bps / 1_000,
            encoder_backend: Some("VideoToolbox-Hardware".to_string()),
            ..CaptureStats::default()
        }
    }

    pub fn stop(&self) {
        let (state, ready) = &*self.pending;
        if let Ok(mut state) = state.lock() {
            state.stopping = true;
            ready.notify_all();
        }
        if let Ok(mut join) = self.join.lock() {
            if let Some(join) = join.take() {
                let _ = join.join();
            }
        }
    }
}

impl Drop for MacEncoderWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_worker(
    mut encoder: H264Encoder,
    pending: Arc<(Mutex<PendingState>, Condvar)>,
    control: Arc<EncoderControlState>,
    telemetry: Arc<WorkerTelemetry>,
    sample_sender: tokio::sync::mpsc::Sender<EncodedVideoSample>,
) {
    loop {
        let input = {
            let (state, ready) = &*pending;
            let Ok(state) = state.lock() else {
                return;
            };
            let Ok((mut state, _)) =
                ready.wait_timeout_while(state, Duration::from_millis(4), |state| {
                    state.input.is_none() && !state.stopping
                })
            else {
                return;
            };
            if state.stopping && state.input.is_none() {
                None
            } else {
                state.input.take()
            }
        };

        if control.take_force_keyframe() {
            let _ = encoder.force_keyframe();
        }
        if let Some(input) = input {
            let result = match input {
                EncoderInput::Cpu(frame) => encoder.encode_frame(&frame).map(|sample| {
                    if let Some(sample) = sample {
                        dispatch_sample(sample, &sample_sender, &telemetry);
                    }
                }),
                EncoderInput::Gpu(surface) => encoder.encode_gpu_surface(surface),
            };
            if let Err(error) = result {
                tracing::warn!(%error, "VideoToolbox worker rejected frame");
                telemetry
                    .frames_encoder_dropped
                    .fetch_add(1, Ordering::Relaxed);
            }
        }

        for sample in encoder.take_encoded_samples() {
            dispatch_sample(sample, &sample_sender, &telemetry);
        }

        let stopping = pending
            .0
            .lock()
            .map(|state| state.stopping && state.input.is_none())
            .unwrap_or(true);
        if stopping {
            encoder.complete_frames();
            for sample in encoder.take_encoded_samples() {
                dispatch_sample(sample, &sample_sender, &telemetry);
            }
            break;
        }
    }
}

fn dispatch_sample(
    sample: EncodedVideoSample,
    sender: &tokio::sync::mpsc::Sender<EncodedVideoSample>,
    telemetry: &WorkerTelemetry,
) {
    if sender.blocking_send(sample).is_err() {
        telemetry
            .frames_encoder_dropped
            .fetch_add(1, Ordering::Relaxed);
    }
}
