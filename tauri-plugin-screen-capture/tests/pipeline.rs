use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use tauri_plugin_screen_capture::{
    capture::FrameConsumer,
    pipeline::{frame::VideoFrame, CapturePipeline},
    publisher::CapturePublisher,
    AnnotationColor, AnnotationDocument, AnnotationElement, AnnotationElementKind, AnnotationPoint,
    CaptureSourceKind, CaptureStats, PixelFormat, Result, StartCaptureOptions,
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

#[derive(Debug)]
struct SlowPublisher {
    pushes: AtomicUsize,
    frames: tokio::sync::Mutex<Vec<VideoFrame>>,
    delay: Duration,
    supports_gpu_surfaces: bool,
}

#[derive(Debug, Default)]
struct BlockingFirstPublisher {
    pushes: AtomicUsize,
    first_started: tokio::sync::Notify,
    release_first: tokio::sync::Notify,
    frames: tokio::sync::Mutex<Vec<VideoFrame>>,
}

impl SlowPublisher {
    fn new(delay: Duration) -> Self {
        Self {
            pushes: AtomicUsize::new(0),
            frames: tokio::sync::Mutex::new(Vec::new()),
            delay,
            supports_gpu_surfaces: false,
        }
    }

    fn with_gpu_surfaces(mut self) -> Self {
        self.supports_gpu_surfaces = true;
        self
    }
}

async fn wait_for_published_frames(publisher: &SlowPublisher, count: usize) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while publisher.frames.lock().await.len() < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("expected frames were published");
}

#[async_trait]
impl CapturePublisher for SlowPublisher {
    fn supports_gpu_surfaces(&self) -> bool {
        self.supports_gpu_surfaces
    }

    async fn start(&self, _options: StartCaptureOptions) -> Result<()> {
        Ok(())
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        tokio::time::sleep(self.delay).await;
        self.frames.lock().await.push(frame);
        self.pushes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(CaptureStats::default())
    }
}

#[async_trait]
impl CapturePublisher for BlockingFirstPublisher {
    async fn start(&self, _options: StartCaptureOptions) -> Result<()> {
        Ok(())
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        if self.pushes.fetch_add(1, Ordering::SeqCst) == 0 {
            self.first_started.notify_one();
            self.release_first.notified().await;
        }
        self.frames.lock().await.push(frame);
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(CaptureStats::default())
    }
}

#[tokio::test]
async fn pipeline_accepts_frames_without_waiting_for_slow_publisher() {
    let publisher = Arc::new(SlowPublisher::new(Duration::from_millis(200)));
    let pipeline = CapturePipeline::new(publisher);

    tokio::time::timeout(Duration::from_millis(50), pipeline.push_frame(frame(1)))
        .await
        .expect("pipeline should not block capture on slow publishing")
        .expect("push frame");
}

#[tokio::test]
async fn pipeline_reports_replaced_frames_as_pipeline_stage_drops() {
    let publisher = Arc::new(SlowPublisher::new(Duration::from_millis(200)));
    let pipeline = CapturePipeline::new(publisher);

    pipeline.push_frame(frame(1)).await.expect("first frame");
    pipeline.push_frame(frame(2)).await.expect("second frame");
    pipeline.push_frame(frame(3)).await.expect("third frame");

    let stats = pipeline.stats().await;
    assert!(stats.frames_pipeline_dropped >= 1);
    assert_eq!(stats.frames_dropped, stats.frames_pipeline_dropped);
    assert!(stats.capture_fps > 0.0);
}

#[allow(dead_code)]
fn start_options() -> StartCaptureOptions {
    StartCaptureOptions {
        source_id: "display:1".to_string(),
        source_kind: CaptureSourceKind::Display,
        fps: Some(30),
        width: Some(2),
        height: Some(2),
        capture_cursor: Some(true),
        publisher: None,
        annotations: None,
    }
}

#[tokio::test]
async fn pipeline_advertises_gpu_surfaces_only_when_publisher_supports_them() {
    let publisher = Arc::new(SlowPublisher::new(Duration::from_millis(1)).with_gpu_surfaces());
    let pipeline = CapturePipeline::new(publisher);

    assert!(pipeline.supports_gpu_surfaces());
}

#[tokio::test]
async fn annotation_enabled_pipeline_composites_the_document_before_publishing() {
    let publisher = Arc::new(SlowPublisher::new(Duration::ZERO).with_gpu_surfaces());
    let pipeline = CapturePipeline::new_with_annotations(publisher.clone());
    pipeline
        .set_annotation_document(AnnotationDocument {
            visible: true,
            elements: vec![AnnotationElement {
                id: "line-1".to_string(),
                kind: AnnotationElementKind::Line,
                points: vec![
                    AnnotationPoint { x: 0.1, y: 0.5 },
                    AnnotationPoint { x: 0.9, y: 0.5 },
                ],
                color: AnnotationColor {
                    red: 255,
                    green: 0,
                    blue: 0,
                    alpha: 255,
                },
                width: 0.1,
            }],
        })
        .await
        .expect("set annotation document");

    let black_frame = VideoFrame {
        width: 10,
        height: 10,
        pixel_format: PixelFormat::Bgra,
        timestamp_ns: 1,
        data: Arc::from([0_u8; 400]),
    };
    pipeline.push_frame(black_frame).await.expect("push frame");

    wait_for_published_frames(&publisher, 1).await;

    let frames = publisher.frames.lock().await;
    let center = (5 * 10 + 5) * 4;
    assert_eq!(&frames[0].data[center..center + 4], &[0, 0, 255, 255]);
    assert!(!pipeline.supports_gpu_surfaces());
}

#[tokio::test]
async fn updating_annotations_republishes_the_latest_static_capture_frame() {
    let publisher = Arc::new(SlowPublisher::new(Duration::ZERO));
    let pipeline = CapturePipeline::new_with_annotations(publisher.clone());
    let black_frame = VideoFrame {
        width: 10,
        height: 10,
        pixel_format: PixelFormat::Bgra,
        timestamp_ns: 1,
        data: Arc::from([0_u8; 400]),
    };
    pipeline.push_frame(black_frame).await.expect("push frame");
    wait_for_published_frames(&publisher, 1).await;

    pipeline
        .set_annotation_document(AnnotationDocument {
            visible: true,
            elements: vec![AnnotationElement {
                id: "static-line".to_string(),
                kind: AnnotationElementKind::Line,
                points: vec![
                    AnnotationPoint { x: 0.1, y: 0.5 },
                    AnnotationPoint { x: 0.9, y: 0.5 },
                ],
                color: AnnotationColor {
                    red: 255,
                    green: 0,
                    blue: 0,
                    alpha: 255,
                },
                width: 0.1,
            }],
        })
        .await
        .expect("set annotation document");

    wait_for_published_frames(&publisher, 2).await;
    let frames = publisher.frames.lock().await;
    let center = (5 * 10 + 5) * 4;
    assert_eq!(&frames[1].data[center..center + 4], &[0, 0, 255, 255]);
    assert!(frames[1].timestamp_ns >= frames[0].timestamp_ns + 1_000_000);
    assert!(frames[1].timestamp_ns / 1_000_000 > frames[0].timestamp_ns / 1_000_000);
}

#[tokio::test]
async fn annotation_refresh_cannot_be_followed_by_an_older_queued_frame() {
    let publisher = Arc::new(BlockingFirstPublisher::default());
    let pipeline = Arc::new(CapturePipeline::new_with_annotations(publisher.clone()));
    pipeline
        .push_frame(VideoFrame {
            width: 10,
            height: 10,
            pixel_format: PixelFormat::Bgra,
            timestamp_ns: 1,
            data: Arc::from([0_u8; 400]),
        })
        .await
        .expect("push initial frame");
    tokio::time::timeout(Duration::from_secs(1), publisher.first_started.notified())
        .await
        .expect("first publish started");

    let update_pipeline = Arc::clone(&pipeline);
    let update = tokio::spawn(async move {
        update_pipeline
            .set_annotation_document(AnnotationDocument {
                visible: true,
                elements: vec![AnnotationElement {
                    id: "ordered-line".to_string(),
                    kind: AnnotationElementKind::Line,
                    points: vec![
                        AnnotationPoint { x: 0.1, y: 0.5 },
                        AnnotationPoint { x: 0.9, y: 0.5 },
                    ],
                    color: AnnotationColor {
                        red: 255,
                        green: 0,
                        blue: 0,
                        alpha: 255,
                    },
                    width: 0.1,
                }],
            })
            .await
    });
    tokio::task::yield_now().await;
    publisher.release_first.notify_one();
    update
        .await
        .expect("annotation update task")
        .expect("annotation refresh");

    tokio::time::timeout(Duration::from_secs(1), async {
        while publisher.frames.lock().await.len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both frames published");
    let frames = publisher.frames.lock().await;
    let center = (5 * 10 + 5) * 4;
    assert_eq!(
        &frames.last().expect("latest frame").data[center..center + 4],
        &[0, 0, 255, 255]
    );
}

#[tokio::test]
async fn annotation_pipeline_renders_every_supported_element_kind() {
    for kind in [
        AnnotationElementKind::Pen,
        AnnotationElementKind::Line,
        AnnotationElementKind::Rectangle,
        AnnotationElementKind::Ellipse,
        AnnotationElementKind::Arrow,
    ] {
        let publisher = Arc::new(SlowPublisher::new(Duration::ZERO));
        let pipeline = CapturePipeline::new_with_annotations(publisher.clone());
        pipeline
            .set_annotation_document(AnnotationDocument {
                visible: true,
                elements: vec![AnnotationElement {
                    id: format!("{kind:?}"),
                    kind,
                    points: vec![
                        AnnotationPoint { x: 0.2, y: 0.2 },
                        AnnotationPoint { x: 0.8, y: 0.8 },
                    ],
                    color: AnnotationColor {
                        red: 255,
                        green: 0,
                        blue: 0,
                        alpha: 255,
                    },
                    width: 0.04,
                }],
            })
            .await
            .expect("set shape document");
        pipeline
            .push_frame(VideoFrame {
                width: 32,
                height: 32,
                pixel_format: PixelFormat::Bgra,
                timestamp_ns: 1,
                data: Arc::from([0_u8; 4096]),
            })
            .await
            .expect("push shape frame");

        wait_for_published_frames(&publisher, 1).await;
        let frames = publisher.frames.lock().await;
        assert!(
            frames[0]
                .data
                .chunks_exact(4)
                .any(|pixel| pixel == [0, 0, 255, 255]),
            "{kind:?} should produce visible pixels"
        );
    }
}

#[tokio::test]
async fn annotation_pipeline_rejects_non_normalized_coordinates() {
    let publisher = Arc::new(SlowPublisher::new(Duration::ZERO));
    let pipeline = CapturePipeline::new_with_annotations(publisher);
    let error = pipeline
        .set_annotation_document(AnnotationDocument {
            visible: true,
            elements: vec![AnnotationElement {
                id: "invalid".to_string(),
                kind: AnnotationElementKind::Pen,
                points: vec![AnnotationPoint { x: 1.1, y: 0.5 }],
                color: AnnotationColor {
                    red: 255,
                    green: 0,
                    blue: 0,
                    alpha: 255,
                },
                width: 0.01,
            }],
        })
        .await
        .expect_err("reject out-of-range point");

    assert_eq!(
        error.payload().code,
        tauri_plugin_screen_capture::CaptureErrorCode::InvalidAnnotation
    );
}

#[tokio::test]
async fn translucent_annotation_blends_each_element_once_per_pixel() {
    let publisher = Arc::new(SlowPublisher::new(Duration::ZERO));
    let pipeline = CapturePipeline::new_with_annotations(publisher.clone());
    pipeline
        .set_annotation_document(AnnotationDocument {
            visible: true,
            elements: vec![AnnotationElement {
                id: "highlighter".to_string(),
                kind: AnnotationElementKind::Line,
                points: vec![
                    AnnotationPoint { x: 0.1, y: 0.5 },
                    AnnotationPoint { x: 0.9, y: 0.5 },
                ],
                color: AnnotationColor {
                    red: 255,
                    green: 0,
                    blue: 0,
                    alpha: 128,
                },
                width: 0.1,
            }],
        })
        .await
        .expect("set translucent annotation");
    pipeline
        .push_frame(VideoFrame {
            width: 10,
            height: 10,
            pixel_format: PixelFormat::Bgra,
            timestamp_ns: 1,
            data: Arc::from([0_u8; 400]),
        })
        .await
        .expect("push frame");
    wait_for_published_frames(&publisher, 1).await;

    let frames = publisher.frames.lock().await;
    let center = (5 * 10 + 5) * 4;
    assert_eq!(&frames[0].data[center..center + 4], &[0, 0, 128, 128]);
}
