const TOOLBAR_WIDTH: f64 = 132.0;
const TOOLBAR_HEIGHT: f64 = 40.0;
const TOOLBAR_GAP: f64 = 8.0;
const HANDLE_RADIUS: f64 = 8.0;
const CLICK_SLOP: f64 = 4.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub(crate) const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub(crate) fn from_points(first: Point, second: Point) -> Self {
        let x = first.x.min(second.x);
        let y = first.y.min(second.y);
        Self {
            x,
            y,
            width: (first.x - second.x).abs(),
            height: (first.y - second.y).abs(),
        }
    }

    pub(crate) fn right(self) -> f64 {
        self.x + self.width
    }

    pub(crate) fn bottom(self) -> f64 {
        self.y + self.height
    }

    pub(crate) fn contains(self, point: Point) -> bool {
        point.x >= self.x
            && point.x <= self.right()
            && point.y >= self.y
            && point.y <= self.bottom()
    }

    fn translated(self, dx: f64, dy: f64, bounds: Size) -> Self {
        Self {
            x: (self.x + dx).clamp(0.0, (bounds.width - self.width).max(0.0)),
            y: (self.y + dy).clamp(0.0, (bounds.height - self.height).max(0.0)),
            ..self
        }
    }

    fn clipped_to(self, bounds: Size) -> Option<Self> {
        if !self.x.is_finite()
            || !self.y.is_finite()
            || !self.width.is_finite()
            || !self.height.is_finite()
            || self.width <= 0.0
            || self.height <= 0.0
        {
            return None;
        }
        let left = self.x.max(0.0);
        let top = self.y.max(0.0);
        let right = self.right().min(bounds.width);
        let bottom = self.bottom().min(bounds.height);
        (right > left && bottom > top).then_some(Self {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub(crate) const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    New {
        anchor: Point,
        click_selection: Option<Rect>,
    },
    Move {
        start: Point,
        initial: Rect,
    },
    Resize {
        fixed: Point,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Ready,
    Dragging,
    Confirming,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Interaction {
    Changed,
    Confirm(Rect),
    Cancel,
    Unchanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolbarButton {
    Cancel,
    Reset,
    Confirm,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ToolbarLayout {
    pub frame: Rect,
}

impl ToolbarLayout {
    pub(crate) fn button_at(self, point: Point) -> Option<ToolbarButton> {
        if !self.frame.contains(point) {
            return None;
        }
        let index = ((point.x - self.frame.x) / (self.frame.width / 3.0))
            .floor()
            .clamp(0.0, 2.0) as u8;
        match index {
            0 => Some(ToolbarButton::Cancel),
            1 => Some(ToolbarButton::Reset),
            2 => Some(ToolbarButton::Confirm),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SelectionModel {
    bounds: Size,
    min_size: Size,
    ui_scale: f64,
    phase: Phase,
    selection: Option<Rect>,
    hovered_window: Option<Rect>,
    drag: Option<Drag>,
}

impl SelectionModel {
    pub(crate) fn new_with_scale(bounds: Size, min_size: Size, ui_scale: f64) -> Self {
        Self {
            bounds,
            min_size,
            ui_scale: if ui_scale.is_finite() && ui_scale > 0.0 {
                ui_scale
            } else {
                1.0
            },
            phase: Phase::Ready,
            selection: None,
            hovered_window: None,
            drag: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn phase(&self) -> Phase {
        self.phase
    }

    pub(crate) fn selection(&self) -> Option<Rect> {
        self.selection
    }

    pub(crate) fn magnifier_visible(&self) -> bool {
        self.phase != Phase::Confirming
    }

    pub(crate) fn magnifier_focus(&self, cursor: Point) -> Option<Point> {
        if !self.magnifier_visible() {
            return None;
        }
        Some(if self.phase == Phase::Dragging {
            self.selection
                .map(|selection| Point::new(selection.right(), selection.bottom()))
                .unwrap_or(cursor)
        } else {
            cursor
        })
    }

    pub(crate) fn preview_selection(&self) -> Option<Rect> {
        if self.phase == Phase::Ready {
            return self.hovered_window;
        }
        if let (
            Some(Drag::New {
                click_selection: Some(window),
                ..
            }),
            Some(selection),
        ) = (self.drag, self.selection)
        {
            let click_slop = CLICK_SLOP * self.ui_scale;
            if selection.width <= click_slop && selection.height <= click_slop {
                return Some(window);
            }
        }
        self.selection
    }

    pub(crate) fn hover_window(&mut self, window: Option<Rect>) -> Interaction {
        if self.phase != Phase::Ready {
            return Interaction::Unchanged;
        }
        let window = window.and_then(|window| window.clipped_to(self.bounds));
        if self.hovered_window == window {
            Interaction::Unchanged
        } else {
            self.hovered_window = window;
            Interaction::Changed
        }
    }

    pub(crate) fn toolbar(&self) -> Option<ToolbarLayout> {
        if self.phase != Phase::Confirming {
            return None;
        }
        let selection = self.selection?;
        let width = (TOOLBAR_WIDTH * self.ui_scale).min(self.bounds.width);
        let height = (TOOLBAR_HEIGHT * self.ui_scale).min(self.bounds.height);
        let x = (selection.right() - width).clamp(0.0, (self.bounds.width - width).max(0.0));
        let toolbar_gap = TOOLBAR_GAP * self.ui_scale;
        let preferred_y = selection.bottom() + toolbar_gap;
        let y = if preferred_y + height <= self.bounds.height {
            preferred_y
        } else {
            (selection.y - toolbar_gap - height).max(0.0)
        };
        Some(ToolbarLayout {
            frame: Rect {
                x,
                y,
                width,
                height,
            },
        })
    }

    pub(crate) fn pointer_down(&mut self, point: Point) -> Interaction {
        let point = self.clamp(point);
        if let Some(button) = self.toolbar().and_then(|toolbar| toolbar.button_at(point)) {
            return match button {
                ToolbarButton::Cancel => Interaction::Cancel,
                ToolbarButton::Reset => {
                    self.reset();
                    Interaction::Changed
                }
                ToolbarButton::Confirm => self.confirm(),
            };
        }

        if self.phase == Phase::Confirming {
            if let Some(selection) = self.selection {
                if let Some(handle) = hit_handle(selection, point, HANDLE_RADIUS * self.ui_scale) {
                    self.drag = Some(Drag::Resize {
                        fixed: opposite_corner(selection, handle),
                    });
                    self.phase = Phase::Dragging;
                    return Interaction::Changed;
                }
                if selection.contains(point) {
                    self.drag = Some(Drag::Move {
                        start: point,
                        initial: selection,
                    });
                    self.phase = Phase::Dragging;
                    return Interaction::Changed;
                }
            }
        }

        let click_selection = self
            .hovered_window
            .filter(|selection| selection.contains(point));
        self.selection = Some(Rect::from_points(point, point));
        self.drag = Some(Drag::New {
            anchor: point,
            click_selection,
        });
        self.phase = Phase::Dragging;
        Interaction::Changed
    }

    pub(crate) fn pointer_move(&mut self, point: Point) -> Interaction {
        let point = self.clamp(point);
        let Some(drag) = self.drag else {
            return Interaction::Unchanged;
        };
        self.selection = Some(match drag {
            Drag::New { anchor, .. } | Drag::Resize { fixed: anchor } => {
                Rect::from_points(anchor, point)
            }
            Drag::Move { start, initial } => {
                initial.translated(point.x - start.x, point.y - start.y, self.bounds)
            }
        });
        Interaction::Changed
    }

    pub(crate) fn pointer_up(&mut self, point: Point) -> Interaction {
        if self.phase != Phase::Dragging {
            return Interaction::Unchanged;
        }
        let _ = self.pointer_move(point);
        let click_selection = match (self.drag, self.selection) {
            (
                Some(Drag::New {
                    click_selection: Some(window),
                    ..
                }),
                Some(selection),
            ) if selection.width <= CLICK_SLOP * self.ui_scale
                && selection.height <= CLICK_SLOP * self.ui_scale =>
            {
                Some(window)
            }
            _ => None,
        };
        self.drag = None;
        if let Some(window) = click_selection {
            self.selection = Some(window);
        }
        match self.selection {
            Some(selection)
                if selection.width >= self.min_size.width
                    && selection.height >= self.min_size.height =>
            {
                self.phase = Phase::Confirming;
                Interaction::Changed
            }
            _ => {
                self.reset();
                Interaction::Changed
            }
        }
    }

    pub(crate) fn confirm(&self) -> Interaction {
        match (self.phase, self.selection) {
            (Phase::Confirming, Some(selection)) => Interaction::Confirm(selection),
            _ => Interaction::Unchanged,
        }
    }

    pub(crate) fn cancel(&self) -> Interaction {
        Interaction::Cancel
    }

    pub(crate) fn reset(&mut self) {
        self.phase = Phase::Ready;
        self.selection = None;
        self.hovered_window = None;
        self.drag = None;
    }

    fn clamp(&self, point: Point) -> Point {
        Point {
            x: point.x.clamp(0.0, self.bounds.width),
            y: point.y.clamp(0.0, self.bounds.height),
        }
    }
}

fn corners(rect: Rect) -> [(Handle, Point); 4] {
    [
        (Handle::TopLeft, Point::new(rect.x, rect.y)),
        (Handle::TopRight, Point::new(rect.right(), rect.y)),
        (Handle::BottomLeft, Point::new(rect.x, rect.bottom())),
        (Handle::BottomRight, Point::new(rect.right(), rect.bottom())),
    ]
}

fn hit_handle(rect: Rect, point: Point, radius: f64) -> Option<Handle> {
    corners(rect).into_iter().find_map(|(handle, corner)| {
        ((point.x - corner.x).abs() <= radius && (point.y - corner.y).abs() <= radius)
            .then_some(handle)
    })
}

fn opposite_corner(rect: Rect, handle: Handle) -> Point {
    match handle {
        Handle::TopLeft => Point::new(rect.right(), rect.bottom()),
        Handle::TopRight => Point::new(rect.x, rect.bottom()),
        Handle::BottomLeft => Point::new(rect.right(), rect.y),
        Handle::BottomRight => Point::new(rect.x, rect.y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> SelectionModel {
        SelectionModel::new_with_scale(Size::new(1000.0, 800.0), Size::new(2.0, 2.0), 1.0)
    }

    #[test]
    fn reverse_drag_is_normalized_and_requires_confirmation() {
        let mut model = model();
        model.pointer_down(Point::new(500.0, 400.0));
        model.pointer_move(Point::new(100.0, 120.0));
        model.pointer_up(Point::new(100.0, 120.0));

        assert_eq!(model.phase(), Phase::Confirming);
        assert_eq!(
            model.selection(),
            Some(Rect {
                x: 100.0,
                y: 120.0,
                width: 400.0,
                height: 280.0,
            })
        );
    }

    #[test]
    fn tiny_drag_resets_instead_of_confirming() {
        let mut model = model();
        model.pointer_down(Point::new(10.0, 10.0));
        model.pointer_up(Point::new(11.0, 11.0));

        assert_eq!(model.phase(), Phase::Ready);
        assert_eq!(model.selection(), None);
    }

    #[test]
    fn selection_can_be_moved_without_leaving_the_display() {
        let mut model = model();
        model.pointer_down(Point::new(100.0, 100.0));
        model.pointer_up(Point::new(300.0, 300.0));
        model.pointer_down(Point::new(200.0, 200.0));
        model.pointer_move(Point::new(990.0, 790.0));
        model.pointer_up(Point::new(990.0, 790.0));

        assert_eq!(
            model.selection(),
            Some(Rect {
                x: 800.0,
                y: 600.0,
                width: 200.0,
                height: 200.0,
            })
        );
    }

    #[test]
    fn corner_handle_resizes_from_the_opposite_corner() {
        let mut model = model();
        model.pointer_down(Point::new(100.0, 100.0));
        model.pointer_up(Point::new(300.0, 300.0));
        model.pointer_down(Point::new(100.0, 100.0));
        model.pointer_up(Point::new(50.0, 40.0));

        assert_eq!(
            model.selection(),
            Some(Rect {
                x: 50.0,
                y: 40.0,
                width: 250.0,
                height: 260.0,
            })
        );
    }

    #[test]
    fn toolbar_buttons_are_stable_near_the_display_edge() {
        let mut model = model();
        model.pointer_down(Point::new(900.0, 700.0));
        model.pointer_up(Point::new(1000.0, 800.0));

        let toolbar = model.toolbar().expect("toolbar");
        assert!(toolbar.frame.x >= 0.0);
        assert!(toolbar.frame.right() <= 1000.0);
        assert!(toolbar.frame.y >= 0.0);
        assert!(toolbar.frame.bottom() <= 800.0);
    }

    #[test]
    fn toolbar_and_handle_hit_area_follow_ui_scale() {
        let mut model =
            SelectionModel::new_with_scale(Size::new(1000.0, 800.0), Size::new(2.0, 2.0), 2.0);
        model.pointer_down(Point::new(100.0, 100.0));
        model.pointer_up(Point::new(400.0, 300.0));

        let toolbar = model.toolbar().expect("toolbar");
        assert_eq!(toolbar.frame.width, TOOLBAR_WIDTH * 2.0);
        assert_eq!(toolbar.frame.height, TOOLBAR_HEIGHT * 2.0);

        model.pointer_down(Point::new(115.0, 115.0));
        model.pointer_up(Point::new(80.0, 70.0));
        assert_eq!(
            model.selection(),
            Some(Rect {
                x: 80.0,
                y: 70.0,
                width: 320.0,
                height: 230.0,
            })
        );
    }

    #[test]
    fn hovered_window_is_previewed_and_committed_by_a_click() {
        let mut model = model();
        let window = Rect {
            x: 120.0,
            y: 80.0,
            width: 640.0,
            height: 480.0,
        };

        model.hover_window(Some(window));
        assert_eq!(model.selection(), None);
        assert_eq!(model.preview_selection(), Some(window));

        model.pointer_down(Point::new(300.0, 200.0));
        model.pointer_up(Point::new(300.0, 200.0));

        assert_eq!(model.phase(), Phase::Confirming);
        assert_eq!(model.selection(), Some(window));
        assert!(model.toolbar().is_some());
    }

    #[test]
    fn dragging_from_a_hovered_window_creates_a_free_selection() {
        let mut model = model();
        model.hover_window(Some(Rect {
            x: 120.0,
            y: 80.0,
            width: 640.0,
            height: 480.0,
        }));

        model.pointer_down(Point::new(300.0, 200.0));
        model.pointer_move(Point::new(500.0, 420.0));
        model.pointer_up(Point::new(500.0, 420.0));

        assert_eq!(
            model.selection(),
            Some(Rect {
                x: 300.0,
                y: 200.0,
                width: 200.0,
                height: 220.0,
            })
        );
    }

    #[test]
    fn magnifier_is_visible_while_selecting_and_hides_during_confirmation() {
        let mut model = model();
        assert!(model.magnifier_visible());

        model.pointer_down(Point::new(100.0, 100.0));
        model.pointer_up(Point::new(300.0, 300.0));
        assert!(!model.magnifier_visible());
        assert_eq!(model.magnifier_focus(Point::new(300.0, 300.0)), None);

        model.reset();
        assert!(model.magnifier_visible());
        assert_eq!(
            model.magnifier_focus(Point::new(300.0, 300.0)),
            Some(Point::new(300.0, 300.0))
        );
    }

    #[test]
    fn magnifier_crosshair_tracks_the_selections_bottom_right_corner() {
        let mut model = model();
        model.pointer_down(Point::new(700.0, 600.0));
        model.pointer_move(Point::new(300.0, 200.0));

        assert_eq!(
            model.magnifier_focus(Point::new(300.0, 200.0)),
            Some(Point::new(700.0, 600.0))
        );
    }
}
