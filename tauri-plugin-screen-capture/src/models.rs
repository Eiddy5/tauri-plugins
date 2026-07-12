use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureSourceKind {
    Display,
    Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionStatus {
    Granted,
    Denied,
    NotDetermined,
    Restricted,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureStatus {
    Idle,
    Starting,
    Capturing,
    Publishing,
    Paused,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PixelFormat {
    Bgra,
    Rgba,
    Nv12,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub platform: String,
    pub supports_display_capture: bool,
    pub supports_window_capture: bool,
    pub supports_thumbnails: bool,
    pub supports_cursor_capture: bool,
    pub supports_webrtc: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSource {
    pub id: String,
    pub kind: CaptureSourceKind,
    pub name: String,
    pub title: Option<String>,
    pub app_name: Option<String>,
    pub pid: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub is_primary: bool,
    pub thumbnail_base64: Option<String>,
    pub filtered_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct ListSourcesOptions {
    pub kinds: Vec<CaptureSourceKind>,
    pub include_thumbnails: bool,
    pub include_current_app: bool,
    pub include_system_ui: bool,
    pub debug_raw_sources: bool,
}

impl Default for ListSourcesOptions {
    fn default() -> Self {
        Self {
            kinds: vec![CaptureSourceKind::Display, CaptureSourceKind::Window],
            include_thumbnails: false,
            include_current_app: false,
            include_system_ui: false,
            debug_raw_sources: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PublisherKind {
    WebRtcLoopback,
    Agora,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgoraPublisherOptions {
    pub app_id: String,
    pub channel: String,
    pub uid: Option<u32>,
    pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublisherOptions {
    pub kind: PublisherKind,
    pub agora: Option<AgoraPublisherOptions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCaptureOptions {
    pub source_id: String,
    pub source_kind: CaptureSourceKind,
    pub fps: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub capture_cursor: Option<bool>,
    pub publisher: Option<PublisherOptions>,
}

impl StartCaptureOptions {
    pub fn effective_fps(&self) -> u32 {
        self.fps.unwrap_or(30).clamp(1, 60)
    }

    pub fn effective_capture_cursor(&self) -> bool {
        self.capture_cursor.unwrap_or(true)
    }

    pub fn effective_video_size(&self) -> (u32, u32) {
        (
            even_video_dimension(self.width.unwrap_or(1280)),
            even_video_dimension(self.height.unwrap_or(720)),
        )
    }

    pub fn with_effective_video_size(mut self) -> Self {
        let (width, height) = self.effective_video_size();
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    pub fn effective_publisher_kind(&self) -> PublisherKind {
        self.publisher
            .as_ref()
            .map(|publisher| publisher.kind)
            .unwrap_or(PublisherKind::WebRtcLoopback)
    }
}

fn even_video_dimension(value: u32) -> u32 {
    value.max(2) & !1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSession {
    pub session_id: String,
    pub source_id: String,
    pub source_kind: CaptureSourceKind,
    pub status: CaptureStatus,
    pub started_at_ms: u128,
    pub last_error: Option<CaptureErrorPayload>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStats {
    pub frames_captured: u64,
    pub frames_published: u64,
    pub frames_dropped: u64,
    #[serde(default)]
    pub frames_capture_dropped: u64,
    #[serde(default)]
    pub frames_pipeline_dropped: u64,
    #[serde(default)]
    pub frames_encoder_dropped: u64,
    #[serde(default)]
    pub frames_cpu_readback: u64,
    pub fps: f64,
    #[serde(default)]
    pub capture_fps: f64,
    #[serde(default)]
    pub publish_fps: f64,
    pub bitrate_kbps: u32,
    #[serde(default)]
    pub encoder_backend: Option<String>,
    pub started: bool,
}

impl Default for CaptureStats {
    fn default() -> Self {
        Self {
            frames_captured: 0,
            frames_published: 0,
            frames_dropped: 0,
            frames_capture_dropped: 0,
            frames_pipeline_dropped: 0,
            frames_encoder_dropped: 0,
            frames_cpu_readback: 0,
            fps: 0.0,
            capture_fps: 0.0,
            publish_fps: 0.0,
            bitrate_kbps: 0,
            encoder_backend: None,
            started: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcOffer {
    pub sdp: String,
    pub sdp_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcAnswer {
    pub sdp: String,
    pub sdp_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcIceCandidate {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureErrorCode {
    PermissionDenied,
    PermissionNotDetermined,
    UnsupportedPlatform,
    SourceNotFound,
    SourceUnavailable,
    CaptureStartFailed,
    CaptureRuntimeFailed,
    PublisherUnsupported,
    #[serde(rename = "webrtcNegotiationFailed")]
    WebRtcNegotiationFailed,
    #[serde(rename = "webrtcTrackFailed")]
    WebRtcTrackFailed,
    InvalidSession,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureErrorPayload {
    pub code: CaptureErrorCode,
    pub message: String,
    pub recoverable: bool,
    pub details: Option<serde_json::Value>,
}
