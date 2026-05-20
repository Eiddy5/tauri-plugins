use base64::{engine::general_purpose::STANDARD, Engine};
use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};

pub fn encode_png_base64(rgba: &[u8], width: u32, height: u32) -> Option<String> {
    let expected_len = width.checked_mul(height)?.checked_mul(4)? as usize;
    if rgba.len() != expected_len {
        return None;
    }

    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(rgba, width, height, ColorType::Rgba8.into())
        .ok()?;

    Some(STANDARD.encode(png))
}

#[cfg(test)]
mod tests {
    use super::encode_png_base64;

    #[test]
    fn encodes_rgba_pixels_as_png_base64() {
        let rgba = [
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 255, 255, 255, 255,
        ];

        let encoded = encode_png_base64(&rgba, 2, 2).expect("png base64");

        assert!(encoded.starts_with("iVBORw0KGgo"));
    }

    #[test]
    fn rejects_rgba_buffers_with_the_wrong_size() {
        assert!(encode_png_base64(&[255, 0, 0, 255], 2, 2).is_none());
    }
}
