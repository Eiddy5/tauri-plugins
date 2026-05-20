use crate::{
    capture::{FrameConsumer, RunningCapture},
    error::Error,
    models::{CaptureErrorCode, CaptureSource, ListSourcesOptions, StartCaptureOptions},
    Result,
};

#[cfg(feature = "macos-screencapturekit")]
mod real {
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    };

    use async_trait::async_trait;
    use screencapturekit::{
        cm::{CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, SCFrameStatus},
        cv::CVPixelBufferLockFlags,
        prelude::{
            PixelFormat as ScPixelFormat, SCContentFilter, SCShareableContent, SCStream,
            SCStreamConfiguration, SCStreamOutputType,
        },
        stream::delegate_trait::StreamCallbacks,
    };

    use super::*;
    use crate::models::{CaptureSourceKind, PixelFormat};
    use crate::sources::{filter_sources, SourceFilterOptions};

    pub async fn list_sources(options: ListSourcesOptions) -> Result<Vec<CaptureSource>> {
        let content = shareable_content()?;
        let snapshot = content.snapshot().ok_or_else(|| {
            Error::new(
                CaptureErrorCode::SourceUnavailable,
                "failed to read ScreenCaptureKit shareable content snapshot",
                true,
            )
        })?;

        let display_count = snapshot.displays.len();
        let window_count = snapshot.windows.len();
        let mut sources = Vec::new();
        if options.kinds.contains(&CaptureSourceKind::Display) {
            sources.extend(
                snapshot
                    .displays
                    .into_iter()
                    .enumerate()
                    .map(|(index, display)| CaptureSource {
                        id: format!("display:{}", display.display_id),
                        kind: CaptureSourceKind::Display,
                        name: format!("Display {}", display.display_id),
                        title: Some(format!("Display {}", display.display_id)),
                        app_name: None,
                        pid: None,
                        width: positive_u32(display.width),
                        height: positive_u32(display.height),
                        scale_factor: 1.0,
                        is_primary: index == 0,
                        thumbnail_base64: None,
                        filtered_reason: None,
                    }),
            );
        }

        if options.kinds.contains(&CaptureSourceKind::Window) {
            sources.extend(snapshot.windows.into_iter().map(|window| {
                let app = window
                    .owning_app_index
                    .and_then(|index| snapshot.applications.get(index));
                let app_name = app.map(|app| app.application_name.clone());
                let pid = app.and_then(|app| u32::try_from(app.process_id).ok());
                let title = window.title.clone();
                let name = title
                    .clone()
                    .filter(|title| !title.is_empty())
                    .or_else(|| app_name.clone())
                    .unwrap_or_else(|| format!("Window {}", window.window_id));

                CaptureSource {
                    id: format!("window:{}", window.window_id),
                    kind: CaptureSourceKind::Window,
                    name,
                    title,
                    app_name,
                    pid,
                    width: positive_f64_u32(window.frame.width),
                    height: positive_f64_u32(window.frame.height),
                    scale_factor: 1.0,
                    is_primary: false,
                    thumbnail_base64: None,
                    filtered_reason: None,
                }
            }));
        }

        let sources = filter_sources(
            sources,
            SourceFilterOptions {
                current_pid: Some(std::process::id()),
                include_current_app: options.include_current_app,
                include_system_ui: options.include_system_ui,
                debug_raw_sources: options.debug_raw_sources,
            },
        );
        eprintln!(
            "[screen-capture] ScreenCaptureKit sources displays={} windows={} filtered={}",
            display_count,
            window_count,
            sources.len()
        );
        Ok(sources)
    }

    pub async fn start_capture(
        options: StartCaptureOptions,
        consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>> {
        eprintln!(
            "[screen-capture] ScreenCaptureKit start source_kind={:?} source_id={} fps={} size={}x{} cursor={}",
            options.source_kind,
            options.source_id,
            options.effective_fps(),
            options.width.unwrap_or(1280),
            options.height.unwrap_or(720),
            options.effective_capture_cursor()
        );
        let content = shareable_content()?;
        let filter = content_filter(&content, &options)?;
        let mut config = SCStreamConfiguration::new()
            .with_width(options.width.unwrap_or(1280))
            .with_height(options.height.unwrap_or(720))
            .with_pixel_format(ScPixelFormat::BGRA)
            .with_fps(options.effective_fps())
            .with_shows_cursor(options.effective_capture_cursor());

        config.set_queue_depth(3);

        let runtime_handle = tokio::runtime::Handle::try_current().map_err(|err| {
            Error::new(
                CaptureErrorCode::CaptureStartFailed,
                format!("failed to capture Tokio runtime handle for ScreenCaptureKit: {err}"),
                true,
            )
        })?;
        let consumer: Arc<dyn FrameConsumer> = Arc::from(consumer);
        let delegate = StreamCallbacks::new()
            .on_error(|error| {
                tracing::error!(?error, "ScreenCaptureKit stream error");
            })
            .on_stop(|error| {
                if let Some(error) = error {
                    tracing::error!(%error, "ScreenCaptureKit stream stopped");
                } else {
                    tracing::debug!("ScreenCaptureKit stream stopped");
                }
            });
        let mut stream = SCStream::new_with_delegate(&filter, &config, delegate);
        let sample_count = Arc::new(AtomicU64::new(0));
        let output_handler_id = stream.add_output_handler(
            {
                let sample_count = Arc::clone(&sample_count);
                move |sample: CMSampleBuffer, output_type: SCStreamOutputType| {
                    if output_type != SCStreamOutputType::Screen {
                        return;
                    }
                    let count = sample_count.fetch_add(1, Ordering::Relaxed) + 1;
                    if count == 1 || count % 30 == 0 {
                        eprintln!(
                            "[screen-capture] ScreenCaptureKit received sample count={count}"
                        );
                    }
                    let Some(frame) = video_frame_from_sample(sample) else {
                        eprintln!(
                        "[screen-capture] ScreenCaptureKit dropped sample before frame conversion"
                    );
                        return;
                    };
                    let consumer = Arc::clone(&consumer);
                    runtime_handle.spawn(async move {
                        if let Err(error) = consumer.push_frame(frame).await {
                            tracing::warn!(?error, "failed to publish ScreenCaptureKit frame");
                            eprintln!(
                            "[screen-capture] failed to publish ScreenCaptureKit frame: {error:?}"
                        );
                        }
                    });
                }
            },
            SCStreamOutputType::Screen,
        );
        if output_handler_id.is_none() {
            return Err(Error::new(
                CaptureErrorCode::CaptureStartFailed,
                "failed to register ScreenCaptureKit output handler",
                true,
            ));
        }

        stream
            .start_capture()
            .map_err(|err| sck_error("failed to start ScreenCaptureKit stream", err))?;
        eprintln!("[screen-capture] ScreenCaptureKit stream started");

        Ok(Box::new(ScreenCaptureKitRunningCapture {
            stream: Mutex::new(stream),
        }))
    }

    struct ScreenCaptureKitRunningCapture {
        stream: Mutex<SCStream>,
    }

    #[async_trait]
    impl RunningCapture for ScreenCaptureKitRunningCapture {
        async fn pause(&self) -> Result<()> {
            self.stream
                .lock()
                .map_err(|_| internal_error("ScreenCaptureKit stream lock was poisoned"))?
                .stop_capture()
                .map_err(|err| sck_error("failed to pause ScreenCaptureKit stream", err))
        }

        async fn resume(&self) -> Result<()> {
            self.stream
                .lock()
                .map_err(|_| internal_error("ScreenCaptureKit stream lock was poisoned"))?
                .start_capture()
                .map_err(|err| sck_error("failed to resume ScreenCaptureKit stream", err))
        }

        async fn stop(&self) -> Result<()> {
            self.stream
                .lock()
                .map_err(|_| internal_error("ScreenCaptureKit stream lock was poisoned"))?
                .stop_capture()
                .map_err(|err| sck_error("failed to stop ScreenCaptureKit stream", err))
        }
    }

    fn shareable_content() -> Result<SCShareableContent> {
        SCShareableContent::create()
            .with_on_screen_windows_only(true)
            .with_exclude_desktop_windows(true)
            .get()
            .map_err(|err| sck_error("failed to read ScreenCaptureKit shareable content", err))
    }

    fn content_filter(
        content: &SCShareableContent,
        options: &StartCaptureOptions,
    ) -> Result<SCContentFilter> {
        match options.source_kind {
            CaptureSourceKind::Display => {
                let display_id = parse_source_id(&options.source_id, "display")?;
                let display = content
                    .displays()
                    .into_iter()
                    .find(|display| display.display_id() == display_id)
                    .ok_or_else(|| source_not_found(&options.source_id))?;
                Ok(SCContentFilter::create()
                    .with_display(&display)
                    .with_excluding_windows(&[])
                    .build())
            }
            CaptureSourceKind::Window => {
                let window_id = parse_source_id(&options.source_id, "window")?;
                let window = content
                    .windows()
                    .into_iter()
                    .find(|window| window.window_id() == window_id)
                    .ok_or_else(|| source_not_found(&options.source_id))?;
                Ok(SCContentFilter::create().with_window(&window).build())
            }
        }
    }

    fn video_frame_from_sample(
        sample: CMSampleBuffer,
    ) -> Option<crate::pipeline::frame::VideoFrame> {
        let frame_status = sample.frame_status();
        if matches!(
            frame_status,
            Some(SCFrameStatus::Blank | SCFrameStatus::Suspended | SCFrameStatus::Stopped)
        ) {
            eprintln!(
                "[screen-capture] ScreenCaptureKit sample has no image content status={frame_status:?}"
            );
            return None;
        }

        let Some(buffer) = sample.image_buffer() else {
            eprintln!(
                "[screen-capture] ScreenCaptureKit sample missing image buffer status={frame_status:?}"
            );
            return None;
        };
        let Ok(guard) = buffer.lock(CVPixelBufferLockFlags::READ_ONLY) else {
            eprintln!(
                "[screen-capture] failed to lock ScreenCaptureKit pixel buffer status={frame_status:?}"
            );
            return None;
        };
        let width = guard.width();
        let height = guard.height();
        let bytes_per_row = guard.bytes_per_row();
        let data = tightly_packed_bgra(guard.as_slice(), width, height, bytes_per_row)?;
        Some(crate::pipeline::frame::VideoFrame {
            width: positive_usize_u32(width),
            height: positive_usize_u32(height),
            pixel_format: PixelFormat::Bgra,
            timestamp_ns: sample.display_time().unwrap_or_default(),
            data,
        })
    }

    fn tightly_packed_bgra(
        source: &[u8],
        width: usize,
        height: usize,
        bytes_per_row: usize,
    ) -> Option<Vec<u8>> {
        let tight_row_len = width.checked_mul(4)?;
        let tight_len = tight_row_len.checked_mul(height)?;
        if bytes_per_row < tight_row_len || source.len() < bytes_per_row.checked_mul(height)? {
            return None;
        }

        if bytes_per_row == tight_row_len {
            return Some(source[..tight_len].to_vec());
        }

        let mut data = Vec::with_capacity(tight_len);
        for row in source.chunks(bytes_per_row).take(height) {
            data.extend_from_slice(&row[..tight_row_len]);
        }
        Some(data)
    }

    fn parse_source_id(source_id: &str, expected_kind: &str) -> Result<u32> {
        let raw_id = source_id
            .strip_prefix(expected_kind)
            .and_then(|suffix| suffix.strip_prefix(':'))
            .ok_or_else(|| source_not_found(source_id))?;
        raw_id
            .parse::<u32>()
            .map_err(|_| source_not_found(source_id))
    }

    fn source_not_found(source_id: &str) -> Error {
        Error::new(
            CaptureErrorCode::SourceNotFound,
            format!("capture source was not found: {source_id}"),
            true,
        )
    }

    fn sck_error(message: &str, error: impl std::fmt::Display) -> Error {
        Error::new(
            CaptureErrorCode::CaptureRuntimeFailed,
            format!("{message}: {error}"),
            true,
        )
    }

    fn internal_error(message: &str) -> Error {
        Error::new(CaptureErrorCode::Internal, message, false)
    }

    fn positive_u32(value: i32) -> u32 {
        u32::try_from(value.max(0)).unwrap_or_default()
    }

    fn positive_f64_u32(value: f64) -> u32 {
        if value.is_finite() && value > 0.0 {
            value.round().min(f64::from(u32::MAX)) as u32
        } else {
            0
        }
    }

    fn positive_usize_u32(value: usize) -> u32 {
        u32::try_from(value).unwrap_or(u32::MAX)
    }
}

#[cfg(feature = "macos-screencapturekit")]
pub use real::{list_sources, start_capture};

#[cfg(not(feature = "macos-screencapturekit"))]
pub async fn list_sources(_options: ListSourcesOptions) -> Result<Vec<CaptureSource>> {
    Ok(Vec::new())
}

#[cfg(not(feature = "macos-screencapturekit"))]
pub async fn start_capture(
    _options: StartCaptureOptions,
    _consumer: Box<dyn FrameConsumer>,
) -> Result<Box<dyn RunningCapture>> {
    Err(Error::new(
        CaptureErrorCode::CaptureStartFailed,
        "macOS ScreenCaptureKit support is not enabled in this build",
        true,
    ))
}
