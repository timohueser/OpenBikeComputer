// The frames of an impostor: directions over the upper half of the sphere, hemi-octahedral.

const FRAMES: f32 = 8.0;

// A point of the unit square from a direction of the upper half sphere.
fn hemi_encode(d: vec3f) -> vec2f {
    let p = d.xy / (abs(d.x) + abs(d.y) + max(d.z, 0.0));
    return vec2f(p.x + p.y, p.x - p.y) * 0.5 + 0.5;
}

fn hemi_decode(uv: vec2f) -> vec3f {
    let e = uv * 2.0 - 1.0;
    let p = vec2f(e.x + e.y, e.x - e.y) * 0.5;
    return normalize(vec3f(p, 1.0 - abs(p.x) - abs(p.y)));
}

// The right and up axes of a view along -d, toward a point from direction d.
fn view_axes(d: vec3f) -> mat2x3f {
    let right = select(vec3f(1.0, 0.0, 0.0), normalize(cross(vec3f(0.0, 0.0, 1.0), d)), abs(d.z) < 0.999);
    return mat2x3f(right, cross(d, right));
}

