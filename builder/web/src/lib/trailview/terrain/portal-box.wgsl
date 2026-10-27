// A point in the box of a tunnel's portal, as `holes` of terrain.ts packs the boxes: per box `a` its
// facade's middle in units about the anchor and its way into the tunnel, `b` in metres its depth, its
// half-width, the heights of its roof's top and of its floor at the facade, and the rise of both per
// metre in. Include it with `// #include portal-box.wgsl`.
// #include drawn-height.wgsl

// A point `u` units about the anchor at height `h` in a box with `a` and `rise`: metres in from its
// facade, metres across from its middle, and its height less the box's rise there.
fn box_point(a: vec4f, rise: f32, u: vec2f, h: f32) -> vec3f {
    let p = (u - a.xy) * surface.unit;
    let along = dot(p, a.zw);
    return vec3f(along, abs(a.z * p.y - a.w * p.x), h - rise * along);
}
