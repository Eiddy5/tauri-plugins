use crate::selection::{Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DesktopRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl DesktopRect {
    pub(crate) const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn union_all(rects: impl IntoIterator<Item = Self>) -> Option<Self> {
        let mut rects = rects.into_iter().filter(|rect| rect.is_valid());
        let first = rects.next()?;
        let (mut left, mut top) = (first.x, first.y);
        let (mut right, mut bottom) = (first.right(), first.bottom());
        for rect in rects {
            left = left.min(rect.x);
            top = top.min(rect.y);
            right = right.max(rect.right());
            bottom = bottom.max(rect.bottom());
        }
        Some(Self::new(left, top, right - left, bottom - top))
    }

    pub(crate) fn right(self) -> f64 {
        self.x + self.width
    }

    pub(crate) fn bottom(self) -> f64 {
        self.y + self.height
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn is_valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }

    fn intersection(self, other: Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > left && bottom > top).then_some(Self::new(left, top, right - left, bottom - top))
    }

    fn local_rect(self, absolute: Self) -> Option<Rect> {
        let clipped = self.intersection(absolute)?;
        Some(Rect {
            x: clipped.x - self.x,
            y: clipped.y - self.y,
            width: clipped.width,
            height: clipped.height,
        })
    }

    pub(crate) fn local_point(self, absolute_x: f64, absolute_y: f64) -> Option<Point> {
        let point = Point::new(absolute_x - self.x, absolute_y - self.y);
        (point.x >= 0.0 && point.y >= 0.0 && point.x <= self.width && point.y <= self.height)
            .then_some(point)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct WindowCatalog {
    front_to_back: Vec<Rect>,
}

impl WindowCatalog {
    pub(crate) fn from_absolute(
        desktop: DesktopRect,
        windows: impl IntoIterator<Item = DesktopRect>,
    ) -> Self {
        let front_to_back = windows
            .into_iter()
            .filter_map(|window| desktop.local_rect(window))
            .filter(|window| window.width >= 2.0 && window.height >= 2.0)
            .collect();
        Self { front_to_back }
    }

    pub(crate) fn window_at(&self, point: Point) -> Option<Rect> {
        self.front_to_back
            .iter()
            .copied()
            .find(|window| window.contains(point))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_desktop_includes_displays_with_negative_coordinates() {
        let desktop = DesktopRect::union_all([
            DesktopRect::new(0.0, 0.0, 1920.0, 1080.0),
            DesktopRect::new(-2560.0, -180.0, 2560.0, 1440.0),
        ]);

        assert_eq!(
            desktop,
            Some(DesktopRect::new(-2560.0, -180.0, 4480.0, 1440.0))
        );
    }

    #[test]
    fn window_catalog_clips_to_desktop_and_keeps_front_to_back_order() {
        let desktop = DesktopRect::new(-1000.0, 0.0, 3000.0, 1200.0);
        let catalog = WindowCatalog::from_absolute(
            desktop,
            [
                DesktopRect::new(-200.0, 100.0, 800.0, 700.0),
                DesktopRect::new(-1200.0, 50.0, 1800.0, 900.0),
            ],
        );

        assert_eq!(
            catalog.window_at(Point::new(900.0, 200.0)),
            Some(Rect {
                x: 800.0,
                y: 100.0,
                width: 800.0,
                height: 700.0,
            })
        );
        assert_eq!(
            catalog.window_at(Point::new(50.0, 100.0)),
            Some(Rect {
                x: 0.0,
                y: 50.0,
                width: 1600.0,
                height: 900.0,
            })
        );
    }
}
