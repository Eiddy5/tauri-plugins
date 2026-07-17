use crate::annotation::{AnnotationOperation, AnnotationRgba, Size, Stroke};

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
