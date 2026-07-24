use std::sync::{Arc, Mutex};

use tauri_plugin_screen_capture::{
    pipeline::frame::VideoFrame,
    publisher::{
        CapturePublisher, CapturePublisherFactory, PublisherBundle, WebRtcPublisher,
    },
    webrtc::signaling::WebRtcSignalingState,
    CaptureErrorCode, CaptureStats, Error, PublisherKind, Result, StartCaptureOptions,
};

// Learn more about Tauri commands at https://v2.tauri.app/develop/calling-rust/#commands
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn prepare_annotation_overlay(window: tauri::WebviewWindow) -> std::result::Result<(), String> {
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = &window;

    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSStatusWindowLevel, NSWindow};

        let toolbar = window.clone();
        window
            .run_on_main_thread(move || {
                let ns_window: &NSWindow =
                    unsafe { &*(toolbar.ns_window().expect("toolbar NSWindow").cast()) };
                ns_window.setLevel(NSStatusWindowLevel + 1);
                ns_window.orderFrontRegardless();
            })
            .map_err(|error| error.to_string())?;
    }

    #[cfg(target_os = "windows")]
    {
        use windows::Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE},
        };

        let raw_hwnd = window.hwnd().map_err(|error| error.to_string())?.0;
        let hwnd = HWND(raw_hwnd);
        unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![greet, prepare_annotation_overlay])
        .plugin(
            tauri_plugin_screen_capture::Builder::new()
                .publisher_factory(Arc::new(ExamplePublisherFactory))
                .build(),
        )
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

struct ExamplePublisherFactory;

#[async_trait::async_trait]
impl CapturePublisherFactory for ExamplePublisherFactory {
    async fn create(&self, options: &StartCaptureOptions) -> Result<PublisherBundle> {
        match options.effective_publisher_kind() {
            PublisherKind::WebRtcLoopback => {
                let signaling = WebRtcSignalingState::new().await?;
                let publisher = WebRtcPublisher::new(signaling);
                let webrtc_signaling = publisher.signaling();
                Ok(PublisherBundle::new(Arc::new(publisher), Some(webrtc_signaling)))
            }
            PublisherKind::Agora => {
                let publisher = NativeAgoraPublisher::new(options)?;
                Ok(PublisherBundle::without_webrtc(Arc::new(publisher)))
            }
        }
    }
}

struct NativeAgoraPublisher {
    channel: String,
    stats: Mutex<CaptureStats>,
}

impl NativeAgoraPublisher {
    fn new(options: &StartCaptureOptions) -> Result<Self> {
        let agora = options
            .publisher
            .as_ref()
            .and_then(|publisher| publisher.agora.as_ref())
            .ok_or_else(|| {
                Error::new(
                    CaptureErrorCode::PublisherUnsupported,
                    "Agora publisher requires appId and channel options",
                    true,
                )
            })?;

        Ok(Self {
            channel: agora.channel.clone(),
            stats: Mutex::new(CaptureStats::default()),
        })
    }

    fn sdk_unavailable(&self) -> Error {
        Error::new(
            CaptureErrorCode::PublisherUnsupported,
            format!(
                "example NativeAgoraPublisher received frames for channel '{}' but Agora Native SDK is not linked yet",
                self.channel
            ),
            true,
        )
    }
}

#[async_trait::async_trait]
impl CapturePublisher for NativeAgoraPublisher {
    async fn start(&self, _options: StartCaptureOptions) -> Result<()> {
        Err(self.sdk_unavailable())
    }

    async fn push_frame(&self, _frame: VideoFrame) -> Result<()> {
        self.stats.lock().unwrap().frames_dropped += 1;
        Err(self.sdk_unavailable())
    }

    async fn pause(&self) -> Result<()> {
        self.stats.lock().unwrap().started = false;
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        Err(self.sdk_unavailable())
    }

    async fn stop(&self) -> Result<()> {
        self.stats.lock().unwrap().started = false;
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(self.stats.lock().unwrap().clone())
    }
}
