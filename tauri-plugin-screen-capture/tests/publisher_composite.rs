use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use tauri_plugin_screen_capture::{
    pipeline::frame::VideoFrame,
    publisher::{CapturePublisher, CompositePublisher},
    CaptureStats, PixelFormat, Result, StartCaptureOptions,
};

fn frame() -> VideoFrame {
    VideoFrame {
        width: 2,
        height: 1,
        pixel_format: PixelFormat::Bgra,
        timestamp_ns: 42,
        data: Arc::from([1, 2, 3, 4, 5, 6, 7, 8]),
    }
}

fn start_options() -> StartCaptureOptions {
    StartCaptureOptions {
        source_id: "display:1".to_string(),
        source_kind: tauri_plugin_screen_capture::CaptureSourceKind::Display,
        fps: Some(30),
        width: Some(2),
        height: Some(1),
        capture_cursor: Some(true),
        publisher: None,
        annotations: None,
    }
}

#[derive(Debug, Default)]
struct PublisherProbe {
    starts: AtomicUsize,
    pushes: AtomicUsize,
    pauses: AtomicUsize,
    resumes: AtomicUsize,
    stops: AtomicUsize,
    data_ptrs: Mutex<Vec<usize>>,
    stats: Mutex<CaptureStats>,
}

#[async_trait]
impl CapturePublisher for PublisherProbe {
    async fn start(&self, _options: StartCaptureOptions) -> Result<()> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.stats.lock().unwrap().started = true;
        Ok(())
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        self.pushes.fetch_add(1, Ordering::SeqCst);
        self.data_ptrs
            .lock()
            .unwrap()
            .push(frame.data.as_ptr() as usize);
        self.stats.lock().unwrap().frames_published += 1;
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        self.pauses.fetch_add(1, Ordering::SeqCst);
        self.stats.lock().unwrap().started = false;
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        self.resumes.fetch_add(1, Ordering::SeqCst);
        self.stats.lock().unwrap().started = true;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        self.stats.lock().unwrap().started = false;
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(self.stats.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn composite_publisher_forwards_shared_frame_data_to_all_publishers() {
    let first = Arc::new(PublisherProbe::default());
    let second = Arc::new(PublisherProbe::default());
    let publisher = CompositePublisher::new(vec![first.clone(), second.clone()]);
    let frame = frame();
    let expected_ptr = frame.data.as_ptr() as usize;

    publisher.start(start_options()).await.expect("start");
    publisher.push_frame(frame).await.expect("push");

    assert_eq!(first.pushes.load(Ordering::SeqCst), 1);
    assert_eq!(second.pushes.load(Ordering::SeqCst), 1);
    assert_eq!(first.data_ptrs.lock().unwrap().as_slice(), &[expected_ptr]);
    assert_eq!(second.data_ptrs.lock().unwrap().as_slice(), &[expected_ptr]);

    let stats = publisher.stats().await.expect("stats");
    assert_eq!(stats.frames_published, 2);
    assert!(stats.started);
}

#[tokio::test]
async fn composite_publisher_fans_out_lifecycle_calls() {
    let first = Arc::new(PublisherProbe::default());
    let second = Arc::new(PublisherProbe::default());
    let publisher = CompositePublisher::new(vec![first.clone(), second.clone()]);

    publisher.start(start_options()).await.expect("start");
    publisher.pause().await.expect("pause");
    publisher.resume().await.expect("resume");
    publisher.stop().await.expect("stop");

    assert_eq!(first.starts.load(Ordering::SeqCst), 1);
    assert_eq!(second.starts.load(Ordering::SeqCst), 1);
    assert_eq!(first.pauses.load(Ordering::SeqCst), 1);
    assert_eq!(second.pauses.load(Ordering::SeqCst), 1);
    assert_eq!(first.resumes.load(Ordering::SeqCst), 1);
    assert_eq!(second.resumes.load(Ordering::SeqCst), 1);
    assert_eq!(first.stops.load(Ordering::SeqCst), 1);
    assert_eq!(second.stops.load(Ordering::SeqCst), 1);

    let stats = publisher.stats().await.expect("stats");
    assert!(!stats.started);
}

#[test]
fn composite_requires_every_publisher_to_support_gpu_surfaces() {
    let cpu_only = Arc::new(PublisherProbe::default());
    let publisher = CompositePublisher::new(vec![cpu_only]);

    assert!(!publisher.supports_gpu_surfaces());
}
