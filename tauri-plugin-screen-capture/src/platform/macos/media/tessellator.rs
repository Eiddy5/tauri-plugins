use crate::{
    annotation::{AnnotationOperation, AnnotationRgba, NormalizedPoint, Size, Stroke},
    models::{AnnotationDocument, AnnotationElement, AnnotationElementKind},
    Result,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeVertex {
    pub position: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshBounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

#[derive(Clone, Debug)]
pub struct StrokeMesh {
    vertices: Vec<StrokeVertex>,
    eraser: bool,
}

impl StrokeMesh {
    pub fn vertices(&self) -> &[StrokeVertex] {
        &self.vertices
    }

    pub const fn is_eraser(&self) -> bool {
        self.eraser
    }

    pub fn bounds(&self) -> MeshBounds {
        self.vertices.iter().fold(
            MeshBounds {
                min_x: f32::INFINITY,
                min_y: f32::INFINITY,
                max_x: f32::NEG_INFINITY,
                max_y: f32::NEG_INFINITY,
            },
            |mut bounds, vertex| {
                bounds.min_x = bounds.min_x.min(vertex.position[0]);
                bounds.min_y = bounds.min_y.min(vertex.position[1]);
                bounds.max_x = bounds.max_x.max(vertex.position[0]);
                bounds.max_y = bounds.max_y.max(vertex.position[1]);
                bounds
            },
        )
    }
}

pub fn tessellate_operation(operation: &AnnotationOperation, output: Size) -> StrokeMesh {
    tessellate(operation.stroke(), output, operation.is_eraser())
}

pub fn document_operations(
    document: &AnnotationDocument,
    output: Size,
) -> Result<Vec<AnnotationOperation>> {
    if !document.visible {
        return Ok(Vec::new());
    }
    let mut operations = Vec::new();
    for element in &document.elements {
        append_document_element(&mut operations, element, output)?;
    }
    Ok(operations)
}

fn append_document_element(
    operations: &mut Vec<AnnotationOperation>,
    element: &AnnotationElement,
    output: Size,
) -> Result<()> {
    let Some(first) = element.points.first().map(normalized_point) else {
        return Ok(());
    };
    let last = element.points.last().map(normalized_point).unwrap_or(first);
    let color = Some(AnnotationRgba {
        red: element.color.red,
        green: element.color.green,
        blue: element.color.blue,
        alpha: element.color.alpha,
    });
    let width = element.width as f32;
    let mut push_stroke = |points: Vec<NormalizedPoint>| -> Result<()> {
        operations.push(AnnotationOperation::Pen(Stroke::new(points, width, color)?));
        Ok(())
    };

    match element.kind {
        AnnotationElementKind::Pen => {
            push_stroke(element.points.iter().map(normalized_point).collect())?;
        }
        AnnotationElementKind::Line => push_stroke(vec![first, last])?,
        AnnotationElementKind::Rectangle => push_stroke(vec![
            first,
            NormalizedPoint::new(last.x, first.y),
            last,
            NormalizedPoint::new(first.x, last.y),
            first,
        ])?,
        AnnotationElementKind::Ellipse => {
            const SEGMENTS: usize = 64;
            let center_x = (first.x + last.x) / 2.0;
            let center_y = (first.y + last.y) / 2.0;
            let radius_x = (last.x - first.x).abs() / 2.0;
            let radius_y = (last.y - first.y).abs() / 2.0;
            let points = (0..=SEGMENTS)
                .map(|index| {
                    let angle = std::f32::consts::TAU * index as f32 / SEGMENTS as f32;
                    NormalizedPoint::new(
                        center_x + angle.cos() * radius_x,
                        center_y + angle.sin() * radius_y,
                    )
                })
                .collect();
            push_stroke(points)?;
        }
        AnnotationElementKind::Arrow => {
            push_stroke(vec![first, last])?;
            let dx = (last.x - first.x) * output.width as f32;
            let dy = (last.y - first.y) * output.height as f32;
            let angle = dy.atan2(dx);
            let head = (dx.hypot(dy) * 0.25).clamp(10.0, 30.0);
            for offset in [2.6_f32, -2.6_f32] {
                push_stroke(vec![
                    last,
                    NormalizedPoint::new(
                        last.x + head * (angle + offset).cos() / output.width as f32,
                        last.y + head * (angle + offset).sin() / output.height as f32,
                    ),
                ])?;
            }
        }
    }
    Ok(())
}

fn normalized_point(point: &crate::models::AnnotationPoint) -> NormalizedPoint {
    NormalizedPoint::new(point.x as f32, point.y as f32)
}

pub fn tessellate(stroke: &Stroke, output: Size, eraser: bool) -> StrokeMesh {
    let points = stroke
        .points()
        .iter()
        .map(|point| {
            [
                point.x * output.width as f32,
                point.y * output.height as f32,
            ]
        })
        .collect::<Vec<_>>();
    let radius = stroke.normalized_width() * output.width.min(output.height) as f32 / 2.0;
    let color = color_components(stroke.color(), eraser);
    let mut vertices = Vec::new();

    for pair in points.windows(2) {
        let a = pair[0];
        let b = pair[1];
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let length = dx.hypot(dy);
        if length <= f32::EPSILON {
            continue;
        }
        let nx = -dy / length * radius;
        let ny = dx / length * radius;
        let a0 = [a[0] + nx, a[1] + ny];
        let a1 = [a[0] - nx, a[1] - ny];
        let b0 = [b[0] + nx, b[1] + ny];
        let b1 = [b[0] - nx, b[1] - ny];
        push_triangle(&mut vertices, a0, a1, b0, color);
        push_triangle(&mut vertices, b0, a1, b1, color);
    }

    for point in points {
        push_circle(&mut vertices, point, radius, color);
    }

    StrokeMesh { vertices, eraser }
}

fn color_components(color: Option<AnnotationRgba>, eraser: bool) -> [f32; 4] {
    if eraser {
        return [1.0; 4];
    }
    let color = color.unwrap_or(AnnotationRgba {
        red: 255,
        green: 59,
        blue: 48,
        alpha: 255,
    });
    let alpha = f32::from(color.alpha) / 255.0;
    [
        f32::from(color.red) / 255.0 * alpha,
        f32::from(color.green) / 255.0 * alpha,
        f32::from(color.blue) / 255.0 * alpha,
        alpha,
    ]
}

fn push_circle(vertices: &mut Vec<StrokeVertex>, center: [f32; 2], radius: f32, color: [f32; 4]) {
    const SEGMENTS: usize = 16;
    for index in 0..SEGMENTS {
        let start = std::f32::consts::TAU * index as f32 / SEGMENTS as f32;
        let end = std::f32::consts::TAU * (index + 1) as f32 / SEGMENTS as f32;
        push_triangle(
            vertices,
            center,
            [
                center[0] + start.cos() * radius,
                center[1] + start.sin() * radius,
            ],
            [
                center[0] + end.cos() * radius,
                center[1] + end.sin() * radius,
            ],
            color,
        );
    }
}

fn push_triangle(
    vertices: &mut Vec<StrokeVertex>,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    color: [f32; 4],
) {
    vertices.extend([a, b, c].map(|position| StrokeVertex { position, color }));
}
