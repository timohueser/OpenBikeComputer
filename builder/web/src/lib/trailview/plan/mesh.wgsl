// The ground plan's near mesh with `plan=mesh` (mesh.ts): each cell's faces at their own heights, drawn
// before the terrain with the corridors' fragment stage (PIECE). The terrain's floors keep it under them.
// #include ../roads/corridor.wgsl

struct PlanIn {
    // Frame units (1/16 world unit) from the cell's origin.
    @location(0) at: vec2i,
    @location(1) z: f32,
    // The look word and the place word of roads/piece.ts.
    @location(2) word: u32,
    @location(3) place: u32,
    // Octahedral, in LOD-space axes.
    @location(4) normal: vec2f,
    // The instance: the cell's origin in whole world units.
    @location(5) origin: vec2i,
}

const FRAME_PER_UNIT: f32 = 16.0;

fn octahedral(e: vec2f) -> vec3f {
    var n = vec3f(e, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) {
        n = vec3f((1.0 - abs(n.yx)) * select(vec2f(-1.0), vec2f(1.0), n.xy >= vec2f(0.0)), n.z);
    }
    return normalize(n);
}

@vertex
fn plan_vertex(v: PlanIn) -> CorridorOut {
    let u = vec2f(v.origin - surface.anchor) + vec2f(v.at) / FRAME_PER_UNIT;
    var p = vec3f(to_render(lod_space(u)), v.z - frame.eye);
    p.z -= frame.curvature * dot(p.xy, p.xy);
    var out: CorridorOut;
    out.clip = frame.view_proj * vec4f(p, 1.0);
    // The debug views show the terrain alone, as the corridors do.
    if (frame.debug != 0u) {
        out.clip = vec4f(0.0, 0.0, 2.0, 1.0);
    }
    out.u = u;
    out.relative = p;
    out.altitude = v.z;
    out.word = v.word;
    out.road = vec2f(across_of(v.place), half_of(v.word));
    out.along = f32(v.place >> 12u) * ALONG_STEP;
    out.normal = octahedral(v.normal);
    return out;
}
