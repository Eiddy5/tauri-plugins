use objc2_app_kit::{
    NSBezierPath, NSColor, NSCompositingOperation, NSGraphicsContext, NSImage,
    NSImageInterpolation, NSWindingRule,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::{
    native::magnifier::MagnifierLayout,
    selection::{Point, Rect},
};

const HANDLE_SIZE: f64 = 8.0;

pub(super) fn draw_dimmed_area(selection: Rect, width: f64, height: f64) {
    let path = NSBezierPath::bezierPath();
    path.setWindingRule(NSWindingRule::EvenOdd);
    path.appendBezierPathWithRect(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(width, height),
    ));
    path.appendBezierPathWithRect(top_down_rect(selection, height));
    path.fill();
}

pub(super) fn translate_to_view(rect: Rect, origin: Point) -> Rect {
    Rect {
        x: rect.x - origin.x,
        y: rect.y - origin.y,
        ..rect
    }
}

pub(super) fn clip_to_view(rect: Rect, width: f64, height: f64) -> Option<Rect> {
    let left = rect.x.max(0.0);
    let top = rect.y.max(0.0);
    let right = rect.right().min(width);
    let bottom = rect.bottom().min(height);
    (right > left && bottom > top).then_some(Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

pub(super) fn draw_selection(selection: Rect, view_height: f64, show_handles: bool) {
    let green =
        NSColor::colorWithSRGBRed_green_blue_alpha(49.0 / 255.0, 214.0 / 255.0, 132.0 / 255.0, 1.0);
    green.setStroke();
    green.setFill();
    let native = top_down_rect(selection, view_height);
    let path = NSBezierPath::bezierPathWithRect(native);
    path.setLineWidth(2.0);
    path.stroke();

    if show_handles {
        for point in [
            Point::new(selection.x, selection.y),
            Point::new(selection.right(), selection.y),
            Point::new(selection.x, selection.bottom()),
            Point::new(selection.right(), selection.bottom()),
        ] {
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(
                    point.x - HANDLE_SIZE / 2.0,
                    view_height - point.y - HANDLE_SIZE / 2.0,
                ),
                NSSize::new(HANDLE_SIZE, HANDLE_SIZE),
            ));
        }
    }
}

pub(super) fn draw_toolbar(toolbar: Rect, view_height: f64) {
    let native = top_down_rect(toolbar, view_height);
    let background =
        NSColor::colorWithSRGBRed_green_blue_alpha(25.0 / 255.0, 25.0 / 255.0, 25.0 / 255.0, 0.96);
    background.setFill();
    let path = NSBezierPath::bezierPath();
    path.appendBezierPathWithRoundedRect_xRadius_yRadius(native, 8.0, 8.0);
    path.fill();

    let white = NSColor::whiteColor();
    white.setStroke();
    let button_width = toolbar.width / 3.0;
    let native_mid_y = native.origin.y + native.size.height / 2.0;
    for index in 1..=2 {
        let x = native.origin.x + button_width * f64::from(index);
        let separator = NSBezierPath::bezierPath();
        separator.moveToPoint(NSPoint::new(x, native.origin.y + 8.0));
        separator.lineToPoint(NSPoint::new(x, native.origin.y + native.size.height - 8.0));
        separator.setLineWidth(1.0);
        separator.stroke();
    }

    let cancel_x = native.origin.x + button_width / 2.0;
    let cancel = NSBezierPath::bezierPath();
    cancel.moveToPoint(NSPoint::new(cancel_x - 5.0, native_mid_y - 5.0));
    cancel.lineToPoint(NSPoint::new(cancel_x + 5.0, native_mid_y + 5.0));
    cancel.moveToPoint(NSPoint::new(cancel_x + 5.0, native_mid_y - 5.0));
    cancel.lineToPoint(NSPoint::new(cancel_x - 5.0, native_mid_y + 5.0));
    cancel.setLineWidth(2.0);
    cancel.stroke();

    let reset_x = native.origin.x + button_width * 1.5;
    let reset = NSBezierPath::bezierPathWithRect(NSRect::new(
        NSPoint::new(reset_x - 6.0, native_mid_y - 6.0),
        NSSize::new(12.0, 12.0),
    ));
    reset.setLineWidth(2.0);
    reset.stroke();

    let confirm_x = native.origin.x + button_width * 2.5;
    let confirm = NSBezierPath::bezierPath();
    confirm.moveToPoint(NSPoint::new(confirm_x - 6.0, native_mid_y));
    confirm.lineToPoint(NSPoint::new(confirm_x - 1.0, native_mid_y - 5.0));
    confirm.lineToPoint(NSPoint::new(confirm_x + 7.0, native_mid_y + 6.0));
    confirm.setLineWidth(2.5);
    confirm.stroke();
}

pub(super) fn draw_magnifier(
    image: &NSImage,
    layout: MagnifierLayout,
    origin: Point,
    view_height: f64,
    desktop_height: f64,
) {
    let frame = translate_to_view(layout.frame, origin);
    let native_frame = top_down_rect(frame, view_height);
    let background = NSBezierPath::bezierPath();
    background.appendBezierPathWithRoundedRect_xRadius_yRadius(native_frame, 8.0, 8.0);
    NSColor::whiteColor().setFill();
    background.fill();

    let inset = 3.0;
    let content = Rect {
        x: frame.x + inset,
        y: frame.y + inset,
        width: (frame.width - inset * 2.0).max(0.0),
        height: (frame.height - inset * 2.0).max(0.0),
    };
    let source = NSRect::new(
        NSPoint::new(
            layout.sample.x,
            desktop_height - layout.sample.y - layout.sample.height,
        ),
        NSSize::new(layout.sample.width, layout.sample.height),
    );
    if content.width > 0.0 && content.height > 0.0 {
        let native_content = top_down_rect(content, view_height);
        if let Some(context) = NSGraphicsContext::currentContext() {
            let interpolation = context.imageInterpolation();
            context.setImageInterpolation(NSImageInterpolation::None);
            image.drawInRect_fromRect_operation_fraction(
                native_content,
                source,
                NSCompositingOperation::Copy,
                1.0,
            );
            context.setImageInterpolation(interpolation);
        } else {
            image.drawInRect_fromRect_operation_fraction(
                native_content,
                source,
                NSCompositingOperation::Copy,
                1.0,
            );
        }

        let focus_x =
            content.x + (layout.focus.x - layout.frame.x) / layout.frame.width * content.width;
        let focus_y =
            content.y + (layout.focus.y - layout.frame.y) / layout.frame.height * content.height;
        let native_focus_y = view_height - focus_y;
        let crosshair = NSBezierPath::bezierPath();
        crosshair.moveToPoint(NSPoint::new(focus_x, native_content.origin.y));
        crosshair.lineToPoint(NSPoint::new(
            focus_x,
            native_content.origin.y + native_content.size.height,
        ));
        crosshair.moveToPoint(NSPoint::new(native_content.origin.x, native_focus_y));
        crosshair.lineToPoint(NSPoint::new(
            native_content.origin.x + native_content.size.width,
            native_focus_y,
        ));
        NSColor::colorWithSRGBRed_green_blue_alpha(7.0 / 255.0, 193.0 / 255.0, 96.0 / 255.0, 1.0)
            .setStroke();
        crosshair.setLineWidth(1.5);
        crosshair.stroke();
    }

    NSColor::colorWithSRGBRed_green_blue_alpha(218.0 / 255.0, 218.0 / 255.0, 218.0 / 255.0, 1.0)
        .setStroke();
    background.setLineWidth(1.0);
    background.stroke();
}

pub(super) fn top_down_rect(rect: Rect, view_height: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.x, view_height - rect.y - rect.height),
        NSSize::new(rect.width.max(0.0), rect.height.max(0.0)),
    )
}
