use serde::{Deserialize, Serialize};

/// Options for a native area-selection screenshot.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CaptureOptions {
    /// Minimum accepted selection width in logical pixels. Defaults to `2`.
    ///
    /// Small selections are treated as an unfinished drag. A value of `0` is
    /// normalized to `1`.
    pub min_width: Option<u32>,
    /// Minimum accepted selection height in logical pixels. Defaults to `2`.
    ///
    /// Small selections are treated as an unfinished drag. A value of `0` is
    /// normalized to `1`.
    pub min_height: Option<u32>,
}

impl CaptureOptions {
    pub(crate) fn effective_min_width(&self) -> f64 {
        f64::from(self.min_width.unwrap_or(2).max(1))
    }

    pub(crate) fn effective_min_height(&self) -> f64 {
        f64::from(self.min_height.unwrap_or(2).max(1))
    }
}

/// Pixel region relative to the captured virtual desktop image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRegion {
    /// Physical-pixel X offset from the virtual desktop image's left edge.
    pub x: u32,
    /// Physical-pixel Y offset from the virtual desktop image's top edge.
    pub y: u32,
    /// Region width in physical pixels.
    pub width: u32,
    /// Region height in physical pixels.
    pub height: u32,
}

/// Metadata returned when a screenshot interaction finishes.
///
/// The JavaScript binding consumes the intermediate `capture_id` immediately
/// to retrieve the PNG bytes and exposes a single result to frontend callers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CaptureResponse {
    Captured {
        capture_id: String,
        mime_type: String,
        width: u32,
        height: u32,
        region: CaptureRegion,
    },
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_response_uses_camel_case_fields() {
        let response = CaptureResponse::Captured {
            capture_id: "capture-1".to_owned(),
            mime_type: "image/png".to_owned(),
            width: 20,
            height: 10,
            region: CaptureRegion {
                x: 1,
                y: 2,
                width: 20,
                height: 10,
            },
        };
        let json = serde_json::to_value(response).expect("serialize response");

        assert_eq!(json["status"], "captured");
        assert_eq!(json["captureId"], "capture-1");
        assert_eq!(json["mimeType"], "image/png");
        assert!(json.get("capture_id").is_none());
    }
}
