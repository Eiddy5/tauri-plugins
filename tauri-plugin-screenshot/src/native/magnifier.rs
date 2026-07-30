use crate::selection::{Point, Rect, Size};

const MAGNIFIER_SIZE: f64 = 112.0;
const MAGNIFIER_GAP: f64 = 16.0;
const SAMPLE_SIZE: f64 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MagnifierLayout {
    pub frame: Rect,
    pub sample: Rect,
    pub focus: Point,
}

pub(crate) fn layout_magnifier(
    anchor: Point,
    focus: Point,
    viewport: Rect,
    source: Size,
    ui_scale: f64,
) -> Option<MagnifierLayout> {
    if anchor.x < viewport.x
        || anchor.y < viewport.y
        || anchor.x >= viewport.right()
        || anchor.y >= viewport.bottom()
        || !focus.x.is_finite()
        || !focus.y.is_finite()
        || viewport.width <= 0.0
        || viewport.height <= 0.0
        || source.width <= 0.0
        || source.height <= 0.0
    {
        return None;
    }
    let scale = if ui_scale.is_finite() && ui_scale > 0.0 {
        ui_scale
    } else {
        1.0
    };
    let width = (MAGNIFIER_SIZE * scale).min(viewport.width);
    let height = (MAGNIFIER_SIZE * scale).min(viewport.height);
    let gap = MAGNIFIER_GAP * scale;
    let frame = Rect {
        x: place_axis(anchor.x, viewport.x, viewport.right(), width, gap),
        y: place_axis(anchor.y, viewport.y, viewport.bottom(), height, gap),
        width,
        height,
    };

    let sample_width = (SAMPLE_SIZE * scale).min(source.width);
    let sample_height = (SAMPLE_SIZE * scale).min(source.height);
    let focus = Point::new(
        focus.x.clamp(0.0, source.width),
        focus.y.clamp(0.0, source.height),
    );
    let sample = Rect {
        x: (focus.x - sample_width / 2.0).clamp(0.0, (source.width - sample_width).max(0.0)),
        y: (focus.y - sample_height / 2.0).clamp(0.0, (source.height - sample_height).max(0.0)),
        width: sample_width,
        height: sample_height,
    };
    let focus = Point::new(
        frame.x + ((focus.x - sample.x) / sample.width).clamp(0.0, 1.0) * frame.width,
        frame.y + ((focus.y - sample.y) / sample.height).clamp(0.0, 1.0) * frame.height,
    );
    Some(MagnifierLayout {
        frame,
        sample,
        focus,
    })
}

fn place_axis(cursor: f64, start: f64, end: f64, size: f64, gap: f64) -> f64 {
    let after = cursor + gap;
    let preferred = if after + size <= end {
        after
    } else {
        cursor - gap - size
    };
    preferred.clamp(start, (end - size).max(start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnifier_starts_below_and_to_the_right_of_the_cursor() {
        let layout = layout_magnifier(
            Point::new(300.0, 200.0),
            Point::new(300.0, 200.0),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 800.0,
            },
            Size::new(1000.0, 800.0),
            1.0,
        )
        .expect("magnifier");

        assert_eq!(
            layout.frame,
            Rect {
                x: 316.0,
                y: 216.0,
                width: 112.0,
                height: 112.0,
            }
        );
        assert_eq!(layout.focus, Point::new(372.0, 272.0));
    }

    #[test]
    fn magnifier_flips_and_keeps_the_focus_aligned_near_the_edge() {
        let layout = layout_magnifier(
            Point::new(998.0, 798.0),
            Point::new(998.0, 798.0),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 800.0,
            },
            Size::new(1000.0, 800.0),
            1.0,
        )
        .expect("magnifier");

        assert_eq!(layout.frame.x, 870.0);
        assert_eq!(layout.frame.y, 670.0);
        assert_eq!(layout.sample.x, 986.0);
        assert_eq!(layout.sample.y, 786.0);
        assert_eq!(layout.focus, Point::new(966.0, 766.0));
    }

    #[test]
    fn display_boundaries_render_the_magnifier_on_only_one_overlay() {
        let cursor = Point::new(1920.0, 300.0);
        let source = Size::new(4480.0, 1440.0);
        let left_display = Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let right_display = Rect {
            x: 1920.0,
            y: 0.0,
            width: 2560.0,
            height: 1440.0,
        };

        assert!(layout_magnifier(cursor, cursor, left_display, source, 1.0).is_none());
        let layout =
            layout_magnifier(cursor, cursor, right_display, source, 1.0).expect("right magnifier");
        assert_eq!(layout.frame.x, 1936.0);
        assert!(layout.frame.right() <= right_display.right());
    }

    #[test]
    fn magnifier_can_follow_the_cursor_while_sampling_another_focus_point() {
        let layout = layout_magnifier(
            Point::new(300.0, 200.0),
            Point::new(700.0, 600.0),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 800.0,
            },
            Size::new(1000.0, 800.0),
            1.0,
        )
        .expect("magnifier");

        assert_eq!(layout.frame.x, 316.0);
        assert_eq!(layout.frame.y, 216.0);
        assert_eq!(layout.sample.x, 693.0);
        assert_eq!(layout.sample.y, 593.0);
        assert_eq!(layout.focus, Point::new(372.0, 272.0));
    }
}
