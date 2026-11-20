// A box in which the ground does not draw, as `holes` of terrain.ts packs the boxes (`BOX` of obc-trailview-parts): per
// box `a` the middle of its start in units about the anchor and its unit way in, `b` in metres its depth, its
// half-width, and the heights of its roof and its floor on its start plane, `c` the rise of those per metre in, the
// height of its spring and the slopes of its start and its end. A point `left` metres to the left of its axis lies in it
// from `c.z · left` metres in to `b.x + c.w · left`; its heights rise between those two end planes, by the point's share
// of the way between them times `c.x · b.x`. Over its spring the roof is a half-ellipse from the half-width to the roof.
// A portal's box has its spring at its roof and slopes 0. Include it with `// #include portal-box.wgsl`.
// #include drawn-height.wgsl

// A point `u` units about the anchor at height `h` in a box: metres in from the middle of its start, metres to its left,
// and its height less the box's rise there.
fn box_point(a: vec4f, b: vec4f, c: vec4f, u: vec2f, h: f32) -> vec3f {
    let p = (u - a.xy) * surface.unit;
    let along = dot(p, a.zw);
    let left = -(a.z * p.y - a.w * p.x);
    // The metres in of the point's share: `along` itself where the ends are square.
    let depth = (along - c.z * left) * (b.x / max(b.x + (c.w - c.z) * left, 1e-6));
    return vec3f(along, left, h - c.x * depth);
}

// How far a point of `box_point` lies outside the box in plan, in metres; negative inside.
fn box_plan(b: vec4f, c: vec4f, q: vec3f) -> f32 {
    return max(max(c.z * q.y - q.x, q.x - b.x - c.w * q.y), abs(q.y) - b.y);
}

// How far it lies outside the box's sides and roof, in about metres; negative inside.
fn box_wall(b: vec4f, c: vec4f, q: vec3f) -> f32 {
    var top = q.z - b.z;
    let r = b.z - c.y;
    if (q.z > c.y && r > 0.0) {
        top = (length(vec2f(q.y / b.y, (q.z - c.y) / r)) - 1.0) * min(b.y, r);
    }
    return max(abs(q.y) - b.y, top);
}
