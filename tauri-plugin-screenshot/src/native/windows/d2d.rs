use windows::{
    core::Result as WindowsResult,
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct2D::{
                Common::{
                    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
                },
                D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1HwndRenderTarget,
                ID2D1SolidColorBrush, ID2D1StrokeStyle, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_BITMAP_PROPERTIES,
                D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
                D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
                D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_HARDWARE,
                D2D1_RENDER_TARGET_USAGE_NONE,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
        },
    },
};
use windows_numerics::Vector2;

use crate::selection::{Rect, Size};

use super::{super::magnifier::MagnifierLayout, frame::CapturedFrame};

const HANDLE_SIZE: f64 = 8.0;

pub(super) struct OverlayScene {
    pub selection: Option<Rect>,
    pub show_handles: bool,
    pub toolbar: Option<Rect>,
    pub magnifier: Option<MagnifierLayout>,
    pub ui_scale: f64,
}

pub(super) struct Direct2DOverlay {
    target: ID2D1HwndRenderTarget,
    original: ID2D1Bitmap,
    dimmed: ID2D1Bitmap,
    green: ID2D1SolidColorBrush,
    dark: ID2D1SolidColorBrush,
    white: ID2D1SolidColorBrush,
}

impl Direct2DOverlay {
    pub fn new(hwnd: HWND, frame: &CapturedFrame, dimmed: &[u8]) -> WindowsResult<Self> {
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }?;
        let size = D2D_SIZE_U {
            width: frame.pixel_width(),
            height: frame.pixel_height(),
        };
        let pixel_format = D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_IGNORE,
        };
        let target_properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_HARDWARE,
            pixelFormat: pixel_format,
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: size,
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let target =
            unsafe { factory.CreateHwndRenderTarget(&target_properties, &hwnd_properties) }?;
        let bitmap_properties = D2D1_BITMAP_PROPERTIES {
            pixelFormat: pixel_format,
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let pitch = frame.pixel_width().saturating_mul(4);
        let original = unsafe {
            target.CreateBitmap(
                size,
                Some(frame.pixels().as_ptr().cast()),
                pitch,
                &bitmap_properties,
            )
        }?;
        let dimmed = unsafe {
            target.CreateBitmap(
                size,
                Some(dimmed.as_ptr().cast()),
                pitch,
                &bitmap_properties,
            )
        }?;
        let green = solid_brush(&target, color(49, 214, 132, 255))?;
        let dark = solid_brush(&target, color(25, 25, 25, 245))?;
        let white = solid_brush(&target, color(245, 245, 245, 255))?;

        Ok(Self {
            target,
            original,
            dimmed,
            green,
            dark,
            white,
        })
    }

    pub fn render(&self, scene: &OverlayScene, frame_size: Size) -> WindowsResult<()> {
        unsafe {
            self.target.BeginDraw();
        }
        let full = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: frame_size.width as f32,
            bottom: frame_size.height as f32,
        };
        unsafe {
            self.target.DrawBitmap(
                &self.dimmed,
                Some(&full),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }

        if let Some(selection) = scene.selection {
            let selected = d2d_rect(selection);
            unsafe {
                self.target
                    .PushAxisAlignedClip(&selected, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                self.target.DrawBitmap(
                    &self.original,
                    Some(&full),
                    1.0,
                    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    None,
                );
                self.target.PopAxisAlignedClip();
                self.target.DrawRectangle(
                    &selected,
                    &self.green,
                    scaled(2.0, scene.ui_scale),
                    None::<&ID2D1StrokeStyle>,
                );
            }
            if scene.show_handles {
                self.draw_handles(selection, scene.ui_scale);
            }
        }
        if let Some(toolbar) = scene.toolbar {
            self.draw_toolbar(toolbar, scene.ui_scale);
        }
        if let Some(magnifier) = scene.magnifier {
            self.draw_magnifier(magnifier, scene.ui_scale);
        }

        unsafe { self.target.EndDraw(None, None) }
    }

    fn draw_handles(&self, selection: Rect, ui_scale: f64) {
        let half = scaled(HANDLE_SIZE / 2.0, ui_scale);
        for (x, y) in [
            (selection.x, selection.y),
            (selection.right(), selection.y),
            (selection.x, selection.bottom()),
            (selection.right(), selection.bottom()),
        ] {
            let rect = D2D_RECT_F {
                left: x as f32 - half,
                top: y as f32 - half,
                right: x as f32 + half,
                bottom: y as f32 + half,
            };
            unsafe {
                self.target.FillRectangle(&rect, &self.green);
            }
        }
    }

    fn draw_toolbar(&self, toolbar: Rect, ui_scale: f64) {
        let rect = d2d_rect(toolbar);
        unsafe {
            self.target.FillRectangle(&rect, &self.dark);
        }
        let button_width = toolbar.width / 3.0;
        for index in 1..=2 {
            let x = toolbar.x + button_width * f64::from(index);
            self.line(
                x,
                toolbar.y + 8.0 * ui_scale,
                x,
                toolbar.bottom() - 8.0 * ui_scale,
                &self.white,
                scaled(1.0, ui_scale),
            );
        }

        let middle_y = toolbar.y + toolbar.height / 2.0;
        let cancel_x = toolbar.x + button_width / 2.0;
        let icon = 6.0 * ui_scale;
        self.line(
            cancel_x - icon,
            middle_y - icon,
            cancel_x + icon,
            middle_y + icon,
            &self.white,
            scaled(2.0, ui_scale),
        );
        self.line(
            cancel_x + icon,
            middle_y - icon,
            cancel_x - icon,
            middle_y + icon,
            &self.white,
            scaled(2.0, ui_scale),
        );

        let reset_x = toolbar.x + button_width * 1.5;
        let reset = D2D_RECT_F {
            left: (reset_x - icon) as f32,
            top: (middle_y - icon) as f32,
            right: (reset_x + icon) as f32,
            bottom: (middle_y + icon) as f32,
        };
        unsafe {
            self.target.DrawRectangle(
                &reset,
                &self.white,
                scaled(2.0, ui_scale),
                None::<&ID2D1StrokeStyle>,
            );
        }

        let confirm_x = toolbar.x + button_width * 2.5;
        self.line(
            confirm_x - icon,
            middle_y,
            confirm_x - ui_scale,
            middle_y + 5.0 * ui_scale,
            &self.white,
            scaled(2.0, ui_scale),
        );
        self.line(
            confirm_x - ui_scale,
            middle_y + 5.0 * ui_scale,
            confirm_x + 7.0 * ui_scale,
            middle_y - icon,
            &self.white,
            scaled(2.0, ui_scale),
        );
    }

    fn draw_magnifier(&self, layout: MagnifierLayout, ui_scale: f64) {
        let frame = d2d_rect(layout.frame);
        unsafe {
            self.target.FillRectangle(&frame, &self.white);
        }
        let inset = 3.0 * ui_scale;
        let content = D2D_RECT_F {
            left: frame.left + inset as f32,
            top: frame.top + inset as f32,
            right: frame.right - inset as f32,
            bottom: frame.bottom - inset as f32,
        };
        let sample = d2d_rect(layout.sample);
        unsafe {
            self.target.DrawBitmap(
                &self.original,
                Some(&content),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                Some(&sample),
            );
        }

        let focus_x = f64::from(content.left)
            + (layout.focus.x - layout.frame.x) / layout.frame.width
                * f64::from(content.right - content.left);
        let focus_y = f64::from(content.top)
            + (layout.focus.y - layout.frame.y) / layout.frame.height
                * f64::from(content.bottom - content.top);
        self.line(
            focus_x,
            f64::from(content.top),
            focus_x,
            f64::from(content.bottom),
            &self.green,
            scaled(1.0, ui_scale),
        );
        self.line(
            f64::from(content.left),
            focus_y,
            f64::from(content.right),
            focus_y,
            &self.green,
            scaled(1.0, ui_scale),
        );
    }

    fn line(&self, x1: f64, y1: f64, x2: f64, y2: f64, brush: &ID2D1SolidColorBrush, width: f32) {
        unsafe {
            self.target.DrawLine(
                Vector2 {
                    X: x1 as f32,
                    Y: y1 as f32,
                },
                Vector2 {
                    X: x2 as f32,
                    Y: y2 as f32,
                },
                brush,
                width,
                None::<&ID2D1StrokeStyle>,
            );
        }
    }
}

fn solid_brush(
    target: &ID2D1HwndRenderTarget,
    color: D2D1_COLOR_F,
) -> WindowsResult<ID2D1SolidColorBrush> {
    unsafe { target.CreateSolidColorBrush(&color, None) }
}

fn color(red: u8, green: u8, blue: u8, alpha: u8) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(red) / 255.0,
        g: f32::from(green) / 255.0,
        b: f32::from(blue) / 255.0,
        a: f32::from(alpha) / 255.0,
    }
}

fn d2d_rect(rect: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.x as f32,
        top: rect.y as f32,
        right: rect.right() as f32,
        bottom: rect.bottom() as f32,
    }
}

fn scaled(value: f64, ui_scale: f64) -> f32 {
    (value * ui_scale).max(1.0) as f32
}
