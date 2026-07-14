use crate::{
    capture::{FrameConsumer, RunningCapture},
    error::Error,
    models::{CaptureErrorCode, CaptureSource, ListSourcesOptions, StartCaptureOptions},
    Result,
};

#[cfg(feature = "macos-screencapturekit")]
mod real {
    use std::sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    };
    use std::time::Duration;

    use async_trait::async_trait;
    use screencapturekit::{
        cm::{CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, SCFrameStatus},
        cv::CVPixelBufferLockFlags,
        prelude::{
            PixelFormat as ScPixelFormat, SCContentFilter, SCShareableContent, SCStream,
            SCStreamConfiguration, SCStreamOutputType,
        },
        screenshot_manager::SCScreenshotManager,
        shareable_content::{SCDisplay, SCWindow},
        stream::delegate_trait::StreamCallbacks,
    };
    use tokio::{runtime::Handle, sync::Notify, task::JoinHandle};

    use super::*;
    use crate::models::{CaptureSourceKind, PixelFormat};
    use crate::pipeline::frame::VideoFrame;
    use crate::platform::macos::window_filter::macos_window_filtered_reason;
    use crate::sources::{filter_sources, thumbnails::encode_png_base64, SourceFilterOptions};

    const THUMBNAIL_MAX_WIDTH: u32 = 320;
    const THUMBNAIL_MAX_HEIGHT: u32 = 180;

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct FilterWindow {
        id: u32,
        pid: i32,
        title: String,
        registered_overlay: bool,
    }

    fn current_app_window_exceptions(current_pid: i32, windows: &[FilterWindow]) -> Vec<u32> {
        windows
            .iter()
            .filter(|window| {
                window.pid == current_pid
                    && !window.registered_overlay
                    && !window
                        .title
                        .starts_with(crate::overlay::macos::OVERLAY_WINDOW_TITLE_PREFIX)
            })
            .map(|window| window.id)
            .collect()
    }

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
                let has_title = title
                    .as_deref()
                    .is_some_and(|title| !title.trim().is_empty());
                let filtered_reason =
                    macos_window_filtered_reason(window.is_on_screen, app.is_some(), has_title);
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
                    filtered_reason: filtered_reason.map(str::to_string),
                }
            }));
        }

        let mut sources = filter_sources(
            sources,
            SourceFilterOptions {
                current_pid: Some(std::process::id()),
                include_current_app: options.include_current_app,
                include_system_ui: options.include_system_ui,
                debug_raw_sources: options.debug_raw_sources,
            },
        );
        if options.include_thumbnails {
            attach_thumbnails(&content, &mut sources);
        }
        eprintln!(
            "[screen-capture] ScreenCaptureKit sources displays={} windows={} filtered={}",
            display_count,
            window_count,
            sources.len()
        );
        Ok(sources)
    }

    fn attach_thumbnails(content: &SCShareableContent, sources: &mut [CaptureSource]) {
        for source in sources {
            source.thumbnail_base64 = capture_thumbnail(content, source);
        }
    }

    fn capture_thumbnail(content: &SCShareableContent, source: &CaptureSource) -> Option<String> {
        let (width, height) = thumbnail_size(source.width, source.height)?;
        let filter = content_filter_for_source(content, source).ok()?;
        let config = SCStreamConfiguration::new()
            .with_width(width)
            .with_height(height)
            .with_shows_cursor(false);
        let image = SCScreenshotManager::capture_image(&filter, &config).ok()?;
        let rgba = image.rgba_data().ok()?;
        encode_png_base64(&rgba, image.width() as u32, image.height() as u32)
    }

    fn thumbnail_size(width: u32, height: u32) -> Option<(u32, u32)> {
        if width == 0 || height == 0 {
            return None;
        }

        let scale = (f64::from(THUMBNAIL_MAX_WIDTH) / f64::from(width))
            .min(f64::from(THUMBNAIL_MAX_HEIGHT) / f64::from(height))
            .min(1.0);
        let thumbnail_width = (f64::from(width) * scale).round().max(1.0) as u32;
        let thumbnail_height = (f64::from(height) * scale).round().max(1.0) as u32;
        Some((thumbnail_width, thumbnail_height))
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
        let dispatcher = LatestFrameDispatcher::new(Arc::from(consumer), runtime_handle.clone());
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
        let conversion_skip_count = Arc::new(AtomicU64::new(0));
        let output_handler_id = stream.add_output_handler(
            {
                let sample_count = Arc::clone(&sample_count);
                let conversion_skip_count = Arc::clone(&conversion_skip_count);
                let dispatcher = dispatcher.handle.clone();
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
                    match video_frame_from_sample(sample) {
                        FrameConversion::Frame(frame) => {
                            if dispatcher.push(frame) {
                                let replaced = dispatcher.replaced_count();
                                if replaced == 1 || replaced % 30 == 0 {
                                    eprintln!(
                                        "[screen-capture] ScreenCaptureKit replaced queued frame count={replaced}"
                                    );
                                }
                            }
                        }
                        FrameConversion::Skipped(reason) => {
                            let skipped = conversion_skip_count.fetch_add(1, Ordering::Relaxed) + 1;
                            if should_log_conversion_skip(reason, skipped) {
                                eprintln!(
                                    "[screen-capture] ScreenCaptureKit skipped sample before frame conversion reason={reason:?} count={skipped}"
                                );
                            }
                        }
                    }
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

        let stream = Arc::new(Mutex::new(stream));
        let worker_stopped = dispatcher.handle.stopped;
        let filter_worker = (options.source_kind == CaptureSourceKind::Display).then(|| {
            spawn_display_filter_worker(
                runtime_handle,
                Arc::clone(&stream),
                options,
                Arc::clone(&worker_stopped),
            )
        });

        Ok(Box::new(ScreenCaptureKitRunningCapture {
            stream,
            worker: dispatcher.worker,
            filter_worker,
            worker_stopped,
            worker_notify: dispatcher.handle.notify,
        }))
    }

    struct ScreenCaptureKitRunningCapture {
        stream: Arc<Mutex<SCStream>>,
        worker: JoinHandle<()>,
        filter_worker: Option<JoinHandle<()>>,
        worker_stopped: Arc<AtomicBool>,
        worker_notify: Arc<Notify>,
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
            self.stop_worker();
            self.stream
                .lock()
                .map_err(|_| internal_error("ScreenCaptureKit stream lock was poisoned"))?
                .stop_capture()
                .map_err(|err| sck_error("failed to stop ScreenCaptureKit stream", err))
        }
    }

    impl ScreenCaptureKitRunningCapture {
        fn stop_worker(&self) {
            self.worker_stopped.store(true, Ordering::Release);
            self.worker_notify.notify_waiters();
            self.worker.abort();
            if let Some(worker) = &self.filter_worker {
                worker.abort();
            }
        }
    }

    impl Drop for ScreenCaptureKitRunningCapture {
        fn drop(&mut self) {
            self.worker_stopped.store(true, Ordering::Release);
            self.worker_notify.notify_waiters();
            self.worker.abort();
            if let Some(worker) = &self.filter_worker {
                worker.abort();
            }
        }
    }

    fn spawn_display_filter_worker(
        runtime_handle: Handle,
        stream: Arc<Mutex<SCStream>>,
        options: StartCaptureOptions,
        stopped: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        runtime_handle.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;

            loop {
                interval.tick().await;
                if stopped.load(Ordering::Acquire) {
                    break;
                }

                let stream = Arc::clone(&stream);
                let options = options.clone();
                let refresh = tokio::task::spawn_blocking(move || -> Result<()> {
                    let content = shareable_content()?;
                    let filter = content_filter(&content, &options)?;
                    stream
                        .lock()
                        .map_err(|_| internal_error("ScreenCaptureKit stream lock was poisoned"))?
                        .update_content_filter(&filter)
                        .map_err(|error| {
                            sck_error("failed to refresh ScreenCaptureKit display filter", error)
                        })
                })
                .await;

                match refresh {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::warn!(%error, "刷新 ScreenCaptureKit 浮层排除列表失败");
                    }
                    Err(error) => {
                        tracing::warn!(%error, "ScreenCaptureKit 浮层排除任务异常结束");
                    }
                }
            }
        })
    }

    struct LatestFrameDispatcher {
        handle: LatestFrameDispatcherHandle,
        worker: JoinHandle<()>,
    }

    #[derive(Clone)]
    struct LatestFrameDispatcherHandle {
        pending: Arc<Mutex<Option<VideoFrame>>>,
        notify: Arc<Notify>,
        stopped: Arc<AtomicBool>,
        replaced: Arc<AtomicU64>,
    }

    impl LatestFrameDispatcher {
        fn new(consumer: Arc<dyn FrameConsumer>, runtime_handle: Handle) -> Self {
            let pending = Arc::new(Mutex::new(None));
            let notify = Arc::new(Notify::new());
            let stopped = Arc::new(AtomicBool::new(false));
            let replaced = Arc::new(AtomicU64::new(0));
            let worker = runtime_handle_spawn_latest_worker(
                runtime_handle,
                Arc::clone(&pending),
                Arc::clone(&notify),
                Arc::clone(&stopped),
                consumer,
            );

            Self {
                handle: LatestFrameDispatcherHandle {
                    pending,
                    notify,
                    stopped,
                    replaced,
                },
                worker,
            }
        }
    }

    impl LatestFrameDispatcherHandle {
        fn push(&self, frame: VideoFrame) -> bool {
            if self.stopped.load(Ordering::Acquire) {
                return false;
            }

            let replaced = self
                .pending
                .lock()
                .map(|mut pending| pending.replace(frame).is_some())
                .unwrap_or(false);
            if replaced {
                self.replaced.fetch_add(1, Ordering::Relaxed);
            }
            self.notify.notify_one();
            replaced
        }

        fn replaced_count(&self) -> u64 {
            self.replaced.load(Ordering::Relaxed)
        }
    }

    fn runtime_handle_spawn_latest_worker(
        runtime_handle: Handle,
        pending: Arc<Mutex<Option<VideoFrame>>>,
        notify: Arc<Notify>,
        stopped: Arc<AtomicBool>,
        consumer: Arc<dyn FrameConsumer>,
    ) -> JoinHandle<()> {
        runtime_handle.spawn(async move {
            loop {
                notify.notified().await;

                while let Some(frame) = pending.lock().ok().and_then(|mut pending| pending.take()) {
                    if let Err(error) = consumer.push_frame(frame).await {
                        tracing::warn!(?error, "failed to publish ScreenCaptureKit frame");
                        eprintln!(
                            "[screen-capture] failed to publish ScreenCaptureKit frame: {error:?}"
                        );
                    }
                }

                if stopped.load(Ordering::Acquire) {
                    break;
                }
            }
        })
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
                display_content_filter(content, &display)
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

    fn content_filter_for_source(
        content: &SCShareableContent,
        source: &CaptureSource,
    ) -> Result<SCContentFilter> {
        match source.kind {
            CaptureSourceKind::Display => {
                let display_id = parse_source_id(&source.id, "display")?;
                let display = content
                    .displays()
                    .into_iter()
                    .find(|display| display.display_id() == display_id)
                    .ok_or_else(|| source_not_found(&source.id))?;
                display_content_filter(content, &display)
            }
            CaptureSourceKind::Window => {
                let window_id = parse_source_id(&source.id, "window")?;
                let window = content
                    .windows()
                    .into_iter()
                    .find(|window| window.window_id() == window_id)
                    .ok_or_else(|| source_not_found(&source.id))?;
                Ok(SCContentFilter::create().with_window(&window).build())
            }
        }
    }

    fn display_content_filter(
        content: &SCShareableContent,
        display: &SCDisplay,
    ) -> Result<SCContentFilter> {
        let current_pid = i32::try_from(std::process::id()).map_err(|_| {
            Error::new(
                CaptureErrorCode::CaptureStartFailed,
                "当前进程 ID 超出 ScreenCaptureKit 支持范围",
                false,
            )
        })?;
        let applications = content.applications();
        let current_app = applications
            .iter()
            .find(|application| application.process_id() == current_pid)
            .ok_or_else(|| {
                Error::new(
                    CaptureErrorCode::CaptureStartFailed,
                    "无法构造安全的浮层排除策略：ScreenCaptureKit 未返回当前应用",
                    true,
                )
            })?;
        let windows = content.windows();
        let filter_windows = windows
            .iter()
            .filter_map(|window| {
                Some(FilterWindow {
                    id: window.window_id(),
                    pid: window.owning_application()?.process_id(),
                    title: window.title().unwrap_or_default(),
                    registered_overlay: crate::overlay::macos::is_registered_overlay_window(
                        window.window_id(),
                    ),
                })
            })
            .collect::<Vec<_>>();
        let exception_ids = current_app_window_exceptions(current_pid, &filter_windows);
        let exceptions = windows
            .iter()
            .filter(|window| exception_ids.contains(&window.window_id()))
            .collect::<Vec<&SCWindow>>();

        Ok(SCContentFilter::create()
            .with_display(display)
            .with_excluding_applications(&[current_app], &exceptions)
            .build())
    }

    #[derive(Debug, Clone, Copy)]
    enum FrameSkipReason {
        NoImageContent(Option<SCFrameStatus>),
        MissingImageBuffer(Option<SCFrameStatus>),
        PixelBufferLockFailed(Option<SCFrameStatus>),
        InvalidPixelBufferLayout,
    }

    enum FrameConversion {
        Frame(VideoFrame),
        Skipped(FrameSkipReason),
    }

    fn video_frame_from_sample(sample: CMSampleBuffer) -> FrameConversion {
        let frame_status = sample.frame_status();
        if matches!(frame_status, Some(status) if !status.has_content()) {
            return FrameConversion::Skipped(FrameSkipReason::NoImageContent(frame_status));
        }

        let Some(buffer) = sample.image_buffer() else {
            return FrameConversion::Skipped(FrameSkipReason::MissingImageBuffer(frame_status));
        };
        let Ok(guard) = buffer.lock(CVPixelBufferLockFlags::READ_ONLY) else {
            return FrameConversion::Skipped(FrameSkipReason::PixelBufferLockFailed(frame_status));
        };
        let width = guard.width();
        let height = guard.height();
        let bytes_per_row = guard.bytes_per_row();
        let Some(data) = tightly_packed_bgra(guard.as_slice(), width, height, bytes_per_row) else {
            return FrameConversion::Skipped(FrameSkipReason::InvalidPixelBufferLayout);
        };

        FrameConversion::Frame(VideoFrame {
            width: positive_usize_u32(width),
            height: positive_usize_u32(height),
            pixel_format: PixelFormat::Bgra,
            timestamp_ns: sample.display_time().unwrap_or_default(),
            data: data.into(),
        })
    }

    fn should_log_conversion_skip(reason: FrameSkipReason, count: u64) -> bool {
        match reason {
            FrameSkipReason::NoImageContent(Some(SCFrameStatus::Idle)) => {
                count == 1 || count % 300 == 0
            }
            FrameSkipReason::MissingImageBuffer(None) => count == 1 || count % 300 == 0,
            FrameSkipReason::PixelBufferLockFailed(status) => {
                let _ = status;
                count == 1 || count % 30 == 0
            }
            _ => count == 1 || count % 30 == 0,
        }
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn keeps_current_process_non_overlay_windows_as_exceptions() {
            let windows = vec![
                FilterWindow {
                    id: 1,
                    pid: 123,
                    title: "TAURI_SCREEN_CAPTURE_OVERLAY:7:tl".into(),
                    registered_overlay: true,
                },
                FilterWindow {
                    id: 2,
                    pid: 123,
                    title: "设置".into(),
                    registered_overlay: false,
                },
                FilterWindow {
                    id: 3,
                    pid: 456,
                    title: "文档".into(),
                    registered_overlay: false,
                },
                FilterWindow {
                    id: 4,
                    pid: 123,
                    title: String::new(),
                    registered_overlay: true,
                },
            ];

            assert_eq!(current_app_window_exceptions(123, &windows), vec![2]);
        }
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
