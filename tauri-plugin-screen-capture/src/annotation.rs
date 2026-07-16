use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
};

use crate::{
    models::{
        AnnotationColor, AnnotationDocument, AnnotationElement, AnnotationElementKind,
        AnnotationPoint, CaptureErrorCode, PixelFormat,
    },
    pipeline::frame::VideoFrame,
    Error, Result,
};

const MAX_ELEMENTS: usize = 512;
const MAX_POINTS_PER_ELEMENT: usize = 4096;
const MAX_TOTAL_POINTS: usize = 16_384;
const MAX_NORMALIZED_WIDTH: f64 = 0.1;

#[derive(Debug)]
pub(crate) struct AnnotationLayer {
    document: RwLock<Arc<AnnotationDocument>>,
}

impl Default for AnnotationLayer {
    fn default() -> Self {
        Self {
            document: RwLock::new(Arc::new(AnnotationDocument::default())),
        }
    }
}

impl AnnotationLayer {
    pub(crate) fn set_document(&self, document: AnnotationDocument) -> Result<()> {
        validate_document(&document)?;
        *self.document.write().map_err(lock_error)? = Arc::new(document);
        Ok(())
    }

    pub(crate) fn document(&self) -> Result<AnnotationDocument> {
        Ok(self.document.read().map_err(lock_error)?.as_ref().clone())
    }

    pub(crate) fn composite(&self, frame: VideoFrame) -> Result<VideoFrame> {
        let document = {
            let document = self.document.read().map_err(lock_error)?;
            Arc::clone(&document)
        };
        if !document.visible || document.elements.is_empty() {
            return Ok(frame);
        }
        if !matches!(frame.pixel_format, PixelFormat::Bgra | PixelFormat::Rgba) {
            return Err(annotation_error(format!(
                "annotation compositing requires BGRA or RGBA frames, got {:?}",
                frame.pixel_format
            )));
        }

        let expected_len = frame.width as usize * frame.height as usize * 4;
        if frame.data.len() != expected_len {
            return Err(annotation_error(format!(
                "annotation frame has {} bytes, expected {expected_len}",
                frame.data.len()
            )));
        }

        let mut frame = frame;
        let data = Arc::make_mut(&mut frame.data);
        let mut coverage = HashSet::new();
        for element in &document.elements {
            coverage.clear();
            draw_element(
                data,
                frame.width,
                frame.height,
                frame.pixel_format,
                element,
                &mut coverage,
            );
        }

        Ok(frame)
    }
}

fn validate_document(document: &AnnotationDocument) -> Result<()> {
    if document.elements.len() > MAX_ELEMENTS {
        return Err(annotation_error(format!(
            "annotation document exceeds {MAX_ELEMENTS} elements"
        )));
    }

    let mut ids = HashSet::with_capacity(document.elements.len());
    let mut total_points = 0_usize;
    for element in &document.elements {
        if element.id.trim().is_empty() {
            return Err(annotation_error("annotation element id cannot be empty"));
        }
        if !ids.insert(element.id.as_str()) {
            return Err(annotation_error(format!(
                "annotation element id {} is duplicated",
                element.id
            )));
        }
        if element.points.len() > MAX_POINTS_PER_ELEMENT {
            return Err(annotation_error(format!(
                "annotation element {} exceeds {MAX_POINTS_PER_ELEMENT} points",
                element.id
            )));
        }
        total_points = total_points.saturating_add(element.points.len());
        if total_points > MAX_TOTAL_POINTS {
            return Err(annotation_error(format!(
                "annotation document exceeds {MAX_TOTAL_POINTS} total points"
            )));
        }
        let minimum_points = match element.kind {
            AnnotationElementKind::Pen => 1,
            AnnotationElementKind::Line
            | AnnotationElementKind::Rectangle
            | AnnotationElementKind::Ellipse
            | AnnotationElementKind::Arrow => 2,
        };
        if element.points.len() < minimum_points {
            return Err(annotation_error(format!(
                "annotation element {} requires at least {minimum_points} points",
                element.id
            )));
        }
        if !element.width.is_finite()
            || element.width <= 0.0
            || element.width > MAX_NORMALIZED_WIDTH
        {
            return Err(annotation_error(format!(
                "annotation element {} width must be in (0, {MAX_NORMALIZED_WIDTH}]",
                element.id
            )));
        }
        if element.points.iter().any(|point| {
            !point.x.is_finite()
                || !point.y.is_finite()
                || !(0.0..=1.0).contains(&point.x)
                || !(0.0..=1.0).contains(&point.y)
        }) {
            return Err(annotation_error(format!(
                "annotation element {} points must use finite normalized coordinates",
                element.id
            )));
        }
    }
    Ok(())
}

fn draw_element(
    data: &mut [u8],
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    element: &AnnotationElement,
    coverage: &mut HashSet<usize>,
) {
    let radius = (element.width * f64::from(width.min(height)) / 2.0).max(1.0);
    match element.kind {
        AnnotationElementKind::Pen => draw_polyline(
            data,
            width,
            height,
            pixel_format,
            &element.points,
            element.color,
            radius,
            coverage,
        ),
        AnnotationElementKind::Line => draw_segment(
            data,
            width,
            height,
            pixel_format,
            element.points[0],
            element.points[1],
            element.color,
            radius,
            coverage,
        ),
        AnnotationElementKind::Rectangle => {
            let a = element.points[0];
            let b = element.points[1];
            let corners = [
                a,
                AnnotationPoint { x: b.x, y: a.y },
                b,
                AnnotationPoint { x: a.x, y: b.y },
                a,
            ];
            draw_polyline(
                data,
                width,
                height,
                pixel_format,
                &corners,
                element.color,
                radius,
                coverage,
            );
        }
        AnnotationElementKind::Ellipse => draw_ellipse(
            data,
            width,
            height,
            pixel_format,
            element.points[0],
            element.points[1],
            element.color,
            radius,
            coverage,
        ),
        AnnotationElementKind::Arrow => draw_arrow(
            data,
            width,
            height,
            pixel_format,
            element.points[0],
            element.points[1],
            element.color,
            radius,
            coverage,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_polyline(
    data: &mut [u8],
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    points: &[AnnotationPoint],
    color: AnnotationColor,
    radius: f64,
    coverage: &mut HashSet<usize>,
) {
    if points.len() == 1 {
        let (x, y) = pixel_point(points[0], width, height);
        draw_disc(
            data,
            width,
            height,
            pixel_format,
            x,
            y,
            radius,
            color,
            coverage,
        );
        return;
    }
    for pair in points.windows(2) {
        draw_segment(
            data,
            width,
            height,
            pixel_format,
            pair[0],
            pair[1],
            color,
            radius,
            coverage,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_segment(
    data: &mut [u8],
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    start: AnnotationPoint,
    end: AnnotationPoint,
    color: AnnotationColor,
    radius: f64,
    coverage: &mut HashSet<usize>,
) {
    let (start_x, start_y) = pixel_point(start, width, height);
    let (end_x, end_y) = pixel_point(end, width, height);
    let delta_x = end_x - start_x;
    let delta_y = end_y - start_y;
    let steps = delta_x.abs().max(delta_y.abs()).ceil().max(1.0) as usize;
    for step in 0..=steps {
        let progress = step as f64 / steps as f64;
        draw_disc(
            data,
            width,
            height,
            pixel_format,
            start_x + delta_x * progress,
            start_y + delta_y * progress,
            radius,
            color,
            coverage,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_ellipse(
    data: &mut [u8],
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    start: AnnotationPoint,
    end: AnnotationPoint,
    color: AnnotationColor,
    radius: f64,
    coverage: &mut HashSet<usize>,
) {
    const SEGMENTS: usize = 72;
    let center_x = (start.x + end.x) / 2.0;
    let center_y = (start.y + end.y) / 2.0;
    let radius_x = (end.x - start.x).abs() / 2.0;
    let radius_y = (end.y - start.y).abs() / 2.0;
    let points = (0..=SEGMENTS)
        .map(|index| {
            let angle = index as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
            AnnotationPoint {
                x: center_x + radius_x * angle.cos(),
                y: center_y + radius_y * angle.sin(),
            }
        })
        .collect::<Vec<_>>();
    draw_polyline(
        data,
        width,
        height,
        pixel_format,
        &points,
        color,
        radius,
        coverage,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_arrow(
    data: &mut [u8],
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    start: AnnotationPoint,
    end: AnnotationPoint,
    color: AnnotationColor,
    radius: f64,
    coverage: &mut HashSet<usize>,
) {
    draw_segment(
        data,
        width,
        height,
        pixel_format,
        start,
        end,
        color,
        radius,
        coverage,
    );
    let (start_x, start_y) = pixel_point(start, width, height);
    let (end_x, end_y) = pixel_point(end, width, height);
    let angle = (end_y - start_y).atan2(end_x - start_x);
    let maximum_length = f64::from(width.min(height)) * 0.12;
    let minimum_length = (radius * 4.0).min(maximum_length);
    let length =
        ((end_x - start_x).hypot(end_y - start_y) * 0.25).clamp(minimum_length, maximum_length);
    for offset in [2.6, -2.6] {
        let head = AnnotationPoint {
            x: (end_x + length * (angle + offset).cos())
                / f64::from(width.saturating_sub(1).max(1)),
            y: (end_y + length * (angle + offset).sin())
                / f64::from(height.saturating_sub(1).max(1)),
        };
        draw_segment(
            data,
            width,
            height,
            pixel_format,
            end,
            AnnotationPoint {
                x: head.x.clamp(0.0, 1.0),
                y: head.y.clamp(0.0, 1.0),
            },
            color,
            radius,
            coverage,
        );
    }
}

fn pixel_point(point: AnnotationPoint, width: u32, height: u32) -> (f64, f64) {
    (
        point.x * f64::from(width.saturating_sub(1)),
        point.y * f64::from(height.saturating_sub(1)),
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_disc(
    data: &mut [u8],
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    center_x: f64,
    center_y: f64,
    radius: f64,
    color: AnnotationColor,
    coverage: &mut HashSet<usize>,
) {
    let min_x = (center_x - radius).floor().max(0.0) as u32;
    let max_x = (center_x + radius)
        .ceil()
        .min(f64::from(width.saturating_sub(1))) as u32;
    let min_y = (center_y - radius).floor().max(0.0) as u32;
    let max_y = (center_y + radius)
        .ceil()
        .min(f64::from(height.saturating_sub(1))) as u32;
    let radius_squared = radius * radius;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let delta_x = f64::from(x) - center_x;
            let delta_y = f64::from(y) - center_y;
            if delta_x * delta_x + delta_y * delta_y <= radius_squared {
                blend_pixel(data, width, x, y, pixel_format, color, coverage);
            }
        }
    }
}

fn blend_pixel(
    data: &mut [u8],
    width: u32,
    x: u32,
    y: u32,
    pixel_format: PixelFormat,
    color: AnnotationColor,
    coverage: &mut HashSet<usize>,
) {
    let pixel_index = y as usize * width as usize + x as usize;
    if !coverage.insert(pixel_index) {
        return;
    }
    let offset = pixel_index * 4;
    let (red_index, blue_index) = match pixel_format {
        PixelFormat::Bgra => (2, 0),
        PixelFormat::Rgba => (0, 2),
        PixelFormat::Nv12 => return,
    };
    let source_alpha = u32::from(color.alpha);
    let inverse_alpha = 255 - source_alpha;
    data[offset + red_index] = blend_channel(
        data[offset + red_index],
        color.red,
        source_alpha,
        inverse_alpha,
    );
    data[offset + 1] = blend_channel(data[offset + 1], color.green, source_alpha, inverse_alpha);
    data[offset + blue_index] = blend_channel(
        data[offset + blue_index],
        color.blue,
        source_alpha,
        inverse_alpha,
    );
    data[offset + 3] =
        (source_alpha + u32::from(data[offset + 3]) * inverse_alpha / 255).min(255) as u8;
}

fn blend_channel(destination: u8, source: u8, source_alpha: u32, inverse_alpha: u32) -> u8 {
    ((u32::from(source) * source_alpha + u32::from(destination) * inverse_alpha + 127) / 255) as u8
}

fn annotation_error(message: impl Into<String>) -> Error {
    Error::new(CaptureErrorCode::InvalidAnnotation, message, true)
}

fn lock_error(error: impl std::fmt::Display) -> Error {
    annotation_error(format!("annotation document lock failed: {error}"))
}
