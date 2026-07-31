use std::{sync::mpsc, time::Duration};

use objc2_core_foundation::{
    CFArray, CFDictionary, CFMutableData, CFNumber, CFRetained, CFString, CFType, Type,
};
use objc2_core_graphics::{
    kCGNullWindowID, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, CGDisplayBounds, CGError,
    CGEvent, CGGetActiveDisplayList, CGImage, CGRectMakeWithDictionaryRepresentation,
    CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_foundation::{NSError, NSPoint, NSRect, NSSize};
use objc2_image_io::CGImageDestination;
use objc2_screen_capture_kit::SCScreenshotManager;

use crate::{
    error::{Error, ErrorCode},
    models::CaptureRegion,
    native::{
        desktop::{DesktopRect, WindowCatalog},
        NativeCaptureImage,
    },
    selection::{Point, Rect, Size},
    Result,
};

#[derive(Clone, Debug)]
pub(super) struct MacDesktop {
    pub(super) bounds: DesktopRect,
    pub(super) displays: Vec<DesktopRect>,
}

pub(super) fn ensure_screen_capture_permission() -> Result<()> {
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return Ok(());
    }
    if unsafe { CGRequestScreenCaptureAccess() } {
        Ok(())
    } else {
        Err(Error::new(
            ErrorCode::PermissionDenied,
            "未获得 macOS 屏幕录制权限，请在系统设置中授权后重试",
            true,
        ))
    }
}

pub(super) fn virtual_desktop() -> Result<MacDesktop> {
    let mut display_count = 0;
    let status = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut display_count) };
    if status != CGError::Success || display_count == 0 {
        return Err(Error::new(
            ErrorCode::DisplayUnavailable,
            "无法读取 macOS 活跃显示器列表",
            true,
        ));
    }

    let mut displays = vec![0; display_count as usize];
    let status =
        unsafe { CGGetActiveDisplayList(display_count, displays.as_mut_ptr(), &mut display_count) };
    if status != CGError::Success {
        return Err(Error::new(
            ErrorCode::DisplayUnavailable,
            "读取 macOS 活跃显示器边界失败",
            true,
        ));
    }
    displays.truncate(display_count as usize);
    let displays = displays
        .into_iter()
        .map(|display_id| {
            let frame = CGDisplayBounds(display_id);
            DesktopRect::new(
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            )
        })
        .filter(|display| display.is_valid())
        .collect::<Vec<_>>();
    let bounds = DesktopRect::union_all(displays.iter().copied()).ok_or_else(|| {
        Error::new(
            ErrorCode::DisplayUnavailable,
            "macOS 虚拟桌面边界为空",
            true,
        )
    })?;
    Ok(MacDesktop { bounds, displays })
}

pub(super) fn cursor_desktop_point(desktop: DesktopRect) -> Option<Point> {
    let event = CGEvent::new(None)?;
    let point = CGEvent::location(Some(&event));
    desktop.local_point(point.x, point.y)
}

#[derive(Clone)]
pub(super) struct MacCapturedFrame {
    image: CFRetained<CGImage>,
    pixel_width: u32,
    pixel_height: u32,
    logical_width: f64,
    logical_height: f64,
}

impl MacCapturedFrame {
    fn new(image: CFRetained<CGImage>, logical_width: f64, logical_height: f64) -> Result<Self> {
        let pixel_width = u32::try_from(CGImage::width(Some(&image))).map_err(|_| {
            Error::new(
                ErrorCode::CaptureFailed,
                "macOS 截图宽度超出可处理范围",
                false,
            )
        })?;
        let pixel_height = u32::try_from(CGImage::height(Some(&image))).map_err(|_| {
            Error::new(
                ErrorCode::CaptureFailed,
                "macOS 截图高度超出可处理范围",
                false,
            )
        })?;
        if pixel_width == 0
            || pixel_height == 0
            || !logical_width.is_finite()
            || !logical_height.is_finite()
            || logical_width <= 0.0
            || logical_height <= 0.0
        {
            return Err(Error::new(
                ErrorCode::CaptureFailed,
                "macOS 截图帧尺寸无效",
                true,
            ));
        }

        Ok(Self {
            image,
            pixel_width,
            pixel_height,
            logical_width,
            logical_height,
        })
    }

    pub(super) fn preview_image(&self) -> CFRetained<CGImage> {
        self.image.clone()
    }

    pub(super) fn logical_size(&self) -> Size {
        Size::new(self.logical_width, self.logical_height)
    }

    pub(super) fn crop_png(&self, logical_region: Rect) -> Result<NativeCaptureImage> {
        let region = self.physical_region(logical_region)?;
        let crop_rect = NSRect::new(
            NSPoint::new(f64::from(region.x), f64::from(region.y)),
            NSSize::new(f64::from(region.width), f64::from(region.height)),
        );
        let image = CGImage::with_image_in_rect(Some(&self.image), crop_rect).ok_or_else(|| {
            Error::new(
                ErrorCode::CaptureFailed,
                "macOS 无法裁剪选中的截图区域",
                true,
            )
        })?;
        let png = encode_cg_image_png(&image)?;

        Ok(NativeCaptureImage {
            png,
            width: region.width,
            height: region.height,
            region,
        })
    }

    fn physical_region(&self, logical_region: Rect) -> Result<CaptureRegion> {
        if !logical_region.x.is_finite()
            || !logical_region.y.is_finite()
            || !logical_region.width.is_finite()
            || !logical_region.height.is_finite()
            || logical_region.width <= 0.0
            || logical_region.height <= 0.0
        {
            return Err(invalid_selection("选区无效"));
        }

        let scale_x = f64::from(self.pixel_width) / self.logical_width;
        let scale_y = f64::from(self.pixel_height) / self.logical_height;
        let left = (logical_region.x * scale_x)
            .floor()
            .clamp(0.0, f64::from(self.pixel_width)) as u32;
        let top = (logical_region.y * scale_y)
            .floor()
            .clamp(0.0, f64::from(self.pixel_height)) as u32;
        let right = (logical_region.right() * scale_x)
            .ceil()
            .clamp(0.0, f64::from(self.pixel_width)) as u32;
        let bottom = (logical_region.bottom() * scale_y)
            .ceil()
            .clamp(0.0, f64::from(self.pixel_height)) as u32;
        let width = right.saturating_sub(left);
        let height = bottom.saturating_sub(top);
        if width == 0 || height == 0 {
            return Err(invalid_selection("选区映射到物理像素后为空"));
        }

        Ok(CaptureRegion {
            x: left,
            y: top,
            width,
            height,
        })
    }
}

pub(super) fn capture_desktop(desktop: DesktopRect) -> Result<MacCapturedFrame> {
    if !desktop.is_valid() {
        return Err(Error::new(
            ErrorCode::DisplayUnavailable,
            "macOS 虚拟桌面边界无效",
            true,
        ));
    }
    let capture_frame = NSRect::new(
        NSPoint::new(desktop.x, desktop.y),
        NSSize::new(desktop.width, desktop.height),
    );

    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = block2::RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let result = retain_cg_image(image, error);
        let _ = sender.send(result);
    });
    unsafe {
        SCScreenshotManager::captureImageInRect_completionHandler(capture_frame, Some(&completion));
    }
    let image = receiver
        .recv_timeout(Duration::from_secs(15))
        .map_err(|error| {
            Error::new(
                ErrorCode::CaptureFailed,
                format!("等待 ScreenCaptureKit 截图超时：{error}"),
                true,
            )
        })?
        .map_err(|message| Error::new(ErrorCode::CaptureFailed, message, true))?;
    MacCapturedFrame::new(image, desktop.width, desktop.height)
}

fn retain_cg_image(
    image: *mut CGImage,
    error: *mut NSError,
) -> std::result::Result<CFRetained<CGImage>, String> {
    if !error.is_null() {
        let description = unsafe { &*error }.localizedDescription().to_string();
        return Err(format!("ScreenCaptureKit 截图失败：{description}"));
    }
    if image.is_null() {
        return Err("ScreenCaptureKit 返回了空图像".to_owned());
    }

    Ok(unsafe { &*image }.retain())
}

fn encode_cg_image_png(image: &CGImage) -> Result<Vec<u8>> {
    let data = CFMutableData::new(None, 0).ok_or_else(|| {
        Error::new(
            ErrorCode::CaptureFailed,
            "macOS 无法分配 PNG 输出缓冲区",
            true,
        )
    })?;
    let png_type = CFString::from_static_str("public.png");
    let destination = unsafe { CGImageDestination::with_data(&data, &png_type, 1, None) }
        .ok_or_else(|| Error::new(ErrorCode::CaptureFailed, "macOS 无法创建 PNG 编码器", true))?;
    unsafe {
        destination.add_image(image, None);
    }
    if !unsafe { destination.finalize() } {
        return Err(Error::new(
            ErrorCode::CaptureFailed,
            "macOS PNG 编码失败",
            true,
        ));
    }
    Ok(data.to_vec())
}

fn invalid_selection(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidSelection, message, true)
}

pub(super) fn mac_window_catalog(desktop: DesktopRect) -> WindowCatalog {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(window_info) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
        return WindowCatalog::default();
    };
    let window_info: &CFArray<CFDictionary<CFString, CFType>> =
        unsafe { window_info.cast_unchecked() };
    let windows = window_info.iter().filter_map(|entry| {
        let layer = dictionary_number(&entry, unsafe { kCGWindowLayer })?;
        if layer != 0.0 {
            return None;
        }
        if dictionary_number(&entry, unsafe { kCGWindowAlpha }).is_some_and(|alpha| alpha <= 0.01) {
            return None;
        }
        let bounds = entry
            .get(unsafe { kCGWindowBounds })?
            .downcast::<CFDictionary>()
            .ok()?;
        let mut frame = NSRect::default();
        if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut frame) } {
            return None;
        }
        Some(DesktopRect::new(
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height,
        ))
    });
    WindowCatalog::from_absolute(desktop, windows)
}

fn dictionary_number(dictionary: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<f64> {
    dictionary.get(key)?.downcast::<CFNumber>().ok()?.as_f64()
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImageAlphaInfo,
    };

    #[test]
    fn native_crop_encodes_only_the_selected_region() {
        let mut pixels = vec![
            255_u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let color_space = CGColorSpace::new_device_rgb().expect("device RGB color space");
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                2,
                2,
                8,
                8,
                Some(&color_space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .expect("bitmap context");
        let image = CGBitmapContextCreateImage(Some(&context)).expect("CGImage");
        let frame = MacCapturedFrame::new(image, 2.0, 2.0).expect("native frame");

        let result = frame
            .crop_png(Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            })
            .expect("cropped PNG");
        let decoded = image::load_from_memory(&result.png)
            .expect("decode PNG")
            .into_rgba8();

        assert_eq!((result.width, result.height), (1, 1));
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }
}
