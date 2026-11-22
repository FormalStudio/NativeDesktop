// The shape of a rounded corner (`CornerCurve`). A circular corner is a quarter
// circle of the corner radius. A continuous corner, the one macOS draws for its
// windows and for CALayer's continuous corner curve, is a superellipse quadrant
// that reaches further along each edge and bends into it without a kink in
// curvature. The two constants are fitted to a macOS 26 window corner captured
// at 2x: for a 16 pt radius, a 62.5 px extent and an exponent of 4.3 leave
// 0.3 px rms against the closest circle's 1.1 px.
const CONTINUOUS_EXTENT: f32 = 1.95;
const CONTINUOUS_EXPONENT: f32 = 4.35;

struct CornerShape {
    // How far along each edge the corner reaches.
    extent: f32,
    // 2 for a circle.
    exponent: f32,
}

fn corner_shape(radius: f32, half_size: vec2<f32>, continuous: bool) -> CornerShape {
    if (!continuous || radius <= 0.0) {
        return CornerShape(radius, 2.0);
    }
    // Without room for the full extent the corner turns back into the circle,
    // so a quad rounded into a disc or a capsule keeps that shape.
    let room = min(half_size.x, half_size.y);
    let s = saturate((room - radius) / ((CONTINUOUS_EXTENT - 1.0) * radius));
    return CornerShape(mix(radius, CONTINUOUS_EXTENT * radius, s), mix(2.0, CONTINUOUS_EXPONENT, s));
}

// Signed distance to the edge of a rounded corner, negative inside, from the
// point's offset to the corner's centre (`extent` in from both edges).
fn corner_sdf(corner_center_to_point: vec2<f32>, shape: CornerShape) -> f32 {
    let p = corner_center_to_point;
    if (shape.extent == 0.0 || p.x <= 0.0 || p.y <= 0.0) {
        // Along a straight edge, or a square corner.
        return max(p.x, p.y) - shape.extent;
    }
    if (shape.exponent == 2.0) {
        return length(p) - shape.extent;
    }
    // The superellipse's norm, divided by the length of its gradient so the
    // result stays a distance in pixels for antialiasing. That length is never
    // below 2^(1/n - 1/2) on the curve; the clamp only catches the corner's
    // centre, where the norm underflows.
    let u = p / shape.extent;
    let a = pow(u, vec2<f32>(shape.exponent - 1.0));
    let norm = max(pow(dot(a, u), 1.0 / shape.exponent), 1e-4);
    let gradient = max(length(a) / pow(norm, shape.exponent - 1.0), 0.5);
    return (norm - 1.0) * shape.extent / gradient;
}

// Signed distance to the inner edge of a continuous corner's border where the
// two borders meeting at it differ, positive inside: the superellipse with
// `axes` (the extent less each border) for its semi-axes, from the same centre.
fn corner_inner_sdf(corner_center_to_point: vec2<f32>, axes: vec2<f32>, exponent: f32) -> f32 {
    if (axes.x <= 0.0 || axes.y <= 0.0) {
        // A border wider than the corner leaves no curve inside it.
        return -1.0;
    }
    let u = corner_center_to_point / axes;
    let a = pow(u, vec2<f32>(exponent - 1.0));
    let norm = max(pow(dot(a, u), 1.0 / exponent), 1e-4);
    let gradient = length(a / axes) / pow(norm, exponent - 1.0);
    return (1.0 - norm) / max(gradient, 1e-4);
}
