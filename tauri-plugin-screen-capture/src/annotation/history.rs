use super::NormalizedPoint;

pub const MAX_POINTS_PER_OPERATION: usize = 8_192;
pub const MAX_OPERATIONS_PER_SESSION: usize = 4_096;
pub const MAX_POINTS_PER_SESSION: usize = 1_000_000;

pub(crate) fn should_retain_sample(
    retained: &[NormalizedPoint],
    incoming: NormalizedPoint,
    normalized_width: f32,
) -> bool {
    retained
        .last()
        .is_none_or(|previous| previous.distance(incoming) >= (normalized_width / 16.0).max(1e-6))
}

pub fn simplify_points(points: &[NormalizedPoint], epsilon: f32) -> Vec<NormalizedPoint> {
    if points.len() <= 2 || !epsilon.is_finite() || epsilon <= 0.0 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    simplify_range(points, 0, points.len() - 1, epsilon, &mut keep);
    points
        .iter()
        .copied()
        .zip(keep)
        .filter_map(|(point, keep)| keep.then_some(point))
        .collect()
}

fn simplify_range(
    points: &[NormalizedPoint],
    start: usize,
    end: usize,
    epsilon: f32,
    keep: &mut [bool],
) {
    if end <= start + 1 {
        return;
    }
    let mut farthest = None;
    let mut maximum = 0.0_f32;
    for index in start + 1..end {
        let distance = point_segment_distance(points[index], points[start], points[end]);
        if distance > maximum {
            maximum = distance;
            farthest = Some(index);
        }
    }
    if maximum > epsilon {
        let index = farthest.expect("a non-empty range has a farthest point");
        keep[index] = true;
        simplify_range(points, start, index, epsilon, keep);
        simplify_range(points, index, end, epsilon, keep);
    }
}

fn point_segment_distance(
    point: NormalizedPoint,
    start: NormalizedPoint,
    end: NormalizedPoint,
) -> f32 {
    let delta_x = end.x - start.x;
    let delta_y = end.y - start.y;
    let length_squared = delta_x * delta_x + delta_y * delta_y;
    if length_squared <= f32::EPSILON {
        return point.distance(start);
    }
    let progress = (((point.x - start.x) * delta_x + (point.y - start.y) * delta_y)
        / length_squared)
        .clamp(0.0, 1.0);
    point.distance(NormalizedPoint {
        x: start.x + progress * delta_x,
        y: start.y + progress * delta_y,
    })
}
