use tauri::{command, State};

use crate::{
    models::{
        AnnotationDocument, AnnotationInputTarget, AnnotationState, AnnotationTool, Capabilities,
        CaptureSession, CaptureSource, CaptureStats, ListSourcesOptions, PermissionStatus,
        StartCaptureOptions, WebRtcAnswer, WebRtcIceCandidate, WebRtcOffer,
    },
    state::ScreenCaptureState,
    Result,
};

#[command]
pub async fn get_capabilities(state: State<'_, ScreenCaptureState>) -> Result<Capabilities> {
    Ok(state.capabilities())
}

#[command]
pub async fn check_permission(state: State<'_, ScreenCaptureState>) -> Result<PermissionStatus> {
    state.check_permission().await
}

#[command]
pub async fn request_permission(state: State<'_, ScreenCaptureState>) -> Result<PermissionStatus> {
    state.request_permission().await
}

#[command]
pub async fn list_sources(
    state: State<'_, ScreenCaptureState>,
    options: Option<ListSourcesOptions>,
) -> Result<Vec<CaptureSource>> {
    state.list_sources(options.unwrap_or_default()).await
}

#[command]
pub async fn start_capture(
    state: State<'_, ScreenCaptureState>,
    options: StartCaptureOptions,
) -> Result<CaptureSession> {
    state.start_capture(options).await
}

#[command]
pub async fn pause_capture(state: State<'_, ScreenCaptureState>, session_id: String) -> Result<()> {
    state.pause_capture(&session_id).await
}

#[command]
pub async fn resume_capture(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<()> {
    state.resume_capture(&session_id).await
}

#[command]
pub async fn stop_capture(state: State<'_, ScreenCaptureState>, session_id: String) -> Result<()> {
    state.stop_capture(&session_id).await
}

#[command]
pub async fn get_capture_session(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<CaptureSession> {
    state.get_capture_session(&session_id).await
}

#[command]
pub async fn get_capture_stats(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<CaptureStats> {
    state.get_capture_stats(&session_id).await
}

#[command]
pub async fn get_annotation_document(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<AnnotationDocument> {
    state.get_annotation_document(&session_id).await
}

#[command]
pub async fn get_annotation_input_target(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<Option<AnnotationInputTarget>> {
    state.get_annotation_input_target(&session_id).await
}

#[command]
pub async fn set_annotation_document(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
    document: AnnotationDocument,
) -> Result<()> {
    state.set_annotation_document(&session_id, document).await
}

#[command]
pub async fn set_annotation_interaction(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
    enabled: bool,
) -> Result<AnnotationState> {
    state.set_annotation_interaction(&session_id, enabled).await
}

#[command]
pub async fn set_annotation_tool(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
    tool: AnnotationTool,
) -> Result<AnnotationState> {
    state.set_annotation_tool(&session_id, tool).await
}

#[command]
pub async fn undo_annotation(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<AnnotationState> {
    state.undo_annotation(&session_id).await
}

#[command]
pub async fn clear_annotations(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<AnnotationState> {
    state.clear_annotations(&session_id).await
}

#[command]
pub async fn get_annotation_state(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<AnnotationState> {
    state.get_annotation_state(&session_id).await
}

#[command]
pub async fn create_webrtc_offer(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<WebRtcOffer> {
    state.create_webrtc_offer(&session_id).await
}

#[command]
pub async fn accept_webrtc_answer(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
    answer: WebRtcAnswer,
) -> Result<()> {
    state.accept_webrtc_answer(&session_id, answer).await
}

#[command]
pub async fn add_webrtc_ice_candidate(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
    candidate: WebRtcIceCandidate,
) -> Result<()> {
    state.add_webrtc_ice_candidate(&session_id, candidate).await
}
