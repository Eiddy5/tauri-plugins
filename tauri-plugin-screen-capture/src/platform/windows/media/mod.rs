pub(crate) mod encoder_worker;
pub(crate) mod gpu_processor;
mod gpu_surface;
pub(crate) mod media_foundation;

pub use gpu_surface::WindowsGpuSurface;

pub(crate) fn fit_inside_even(
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> (u32, u32) {
    let source_width = u64::from(source_width.max(1));
    let source_height = u64::from(source_height.max(1));
    let target_width = u64::from(target_width.max(2));
    let target_height = u64::from(target_height.max(2));
    let (width, height) = if source_width.saturating_mul(target_height)
        > target_width.saturating_mul(source_height)
    {
        (
            target_width,
            target_width.saturating_mul(source_height) / source_width,
        )
    } else {
        (
            target_height.saturating_mul(source_width) / source_height,
            target_height,
        )
    };
    ((width.max(2) as u32) & !1, (height.max(2) as u32) & !1)
}

pub(crate) fn recommended_screen_share_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    let target_bitrate_bps = pixels
        .saturating_mul(u64::from(fps.max(1)))
        .saturating_mul(8)
        .div_ceil(100);
    let maximum_bitrate_bps = if pixels <= 1920 * 1080 {
        8_000_000
    } else {
        32_000_000
    };
    target_bitrate_bps.clamp(4_000_000, maximum_bitrate_bps) as u32
}

#[cfg(test)]
mod tests {
    use super::{fit_inside_even, recommended_screen_share_bitrate};

    #[test]
    fn fit_inside_even_bounds_landscape_and_portrait_sources() {
        assert_eq!(fit_inside_even(3840, 2160, 1920, 1080), (1920, 1080));
        assert_eq!(fit_inside_even(2160, 3840, 1920, 1080), (606, 1080));
    }

    #[test]
    fn screen_share_bitrate_scales_with_manual_quality() {
        assert_eq!(recommended_screen_share_bitrate(1280, 720, 60), 4_423_680);
        assert_eq!(recommended_screen_share_bitrate(1920, 1080, 60), 8_000_000);
        assert_eq!(recommended_screen_share_bitrate(2560, 1440, 60), 17_694_720);
        assert_eq!(recommended_screen_share_bitrate(3840, 2160, 30), 19_906_560);
        assert_eq!(recommended_screen_share_bitrate(3840, 2160, 60), 32_000_000);
    }
}
