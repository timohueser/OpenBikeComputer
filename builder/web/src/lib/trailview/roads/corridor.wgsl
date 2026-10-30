// PROTOTYPE road system (a study; never merges). The corridor faces of roads/corridor.ts in the structures
// pass: the heights of the band's rows, the range where corridors draw, and the materials. structures.wgsl
// includes it and calls corridor_height in its vertex stage and corridor_fragment in its fragment stage.
// #include hole.wgsl
// #include ../ground/look.wgsl
// #include junctions.wgsl

// The flags in a vertex colour's alpha that mark a corridor vertex, as CORRIDOR of corridor.ts.
const CORRIDOR_MARK: u32 = 12u;
// Corridor vertex flags in the colour's blue, as CF of section.ts: the height is an offset over the
// stamped ground or over the drawn mesh; a barrier. The band's priority is in the upper four bits.
const CF_SAMPLE: u32 = 1u;
const CF_MESH: u32 = 2u;
const CF_BARRIER: u32 = 4u;
// Materials, as MAT of section.ts.
const MAT_GROUND: u32 = 1u;
const MAT_PAINT: u32 = 2u;
const MAT_SHOULDER: u32 = 3u;
const MAT_KERB: u32 = 4u;
const MAT_SIDEWALK: u32 = 5u;
const MAT_BALLAST: u32 = 6u;
const MAT_STEEL: u32 = 7u;
const MAT_POST: u32 = 8u;
const MAT_WALL: u32 = 9u;
const MAT_CONCRETE: u32 = 10u;
// A band's row over another road's carriageway sinks this far, so that road draws over it.
const BAND_UNDER_M: f32 = 0.2;
// Past the corridors' range a corridor sinks this far under the ground, where the mask paints its road.
const CORRIDOR_SINK_M: f32 = 4.0;

// A corridor draws this far toward the eye, plus a share of its distance: where its band and the terrain
// hold the same ground, the band wins, and a hill in front of a road still hides it.
const CORRIDOR_BIAS_M: f32 = 0.06;
const CORRIDOR_BIAS_SHARE: f32 = 0.003;

// A corridor vertex at render-space `p` about the eye, moved toward the eye.
fn corridor_biased(p: vec3f) -> vec3f {
    let d = length(p);
    return p * max(0.0, 1.0 - (CORRIDOR_BIAS_M + CORRIDOR_BIAS_SHARE * d) / max(d, 1e-3));
}

fn is_corridor(colour: vec4f) -> bool {
    return (u32(round(colour.a * 255.0)) & CORRIDOR_MARK) == CORRIDOR_MARK;
}

// The height of a corridor vertex at `at` (whole world units) and LOD-space `s`: its own height, or an
// offset over the stamped ground (the finest level that knows it) or over the drawn mesh. `fallback` is
// the ground under the eye, where no page knows the ground yet.
fn corridor_height(at: vec2i, z: f32, colour: vec4f, s: vec2f, fallback: f32) -> f32 {
    let flags = u32(round(colour.b * 255.0));
    var h = z;
    if ((flags & CF_SAMPLE) != 0u) {
        let ground = units_height(at, surface.finest);
        h = select(fallback, ground, known(ground)) + z + f32(flags >> 4u) * 0.004;
        let m = mask_at(s, u32(ceil(lod(s))), true);
        if (m.level != 0u && m.kind.x != 0u && m.edge < -0.05) {
            h -= BAND_UNDER_M;
        }
    } else if ((flags & CF_MESH) != 0u) {
        let ground = plain_height(s);
        h = select(fallback, ground, known(ground)) + z;
    }
    if (!corridor_drawn(s)) {
        h -= CORRIDOR_SINK_M;
    }
    return h;
}

// The colour of a corridor fragment: `u` units about the anchor, `relative` render space about the eye,
// its altitude, its vertex colour, metres across and half-width (`road`), metres along (on a track, its
// sleepers), and the face's normal.
fn corridor_fragment(u: vec2f, relative: vec3f, altitude: f32, colour: vec4f, road: vec2f, along: f32, n: vec3f) -> vec4f {
    let mat = u32(round(colour.r * 255.0));
    let code = u32(round(colour.g * 255.0)) & 15u;
    let su = lod_space(u);
    let level = u32(ceil(lod(su)));
    if (mat == MAT_GROUND) {
        return vec4f(ground_colour(su, relative, altitude, level), 1.0);
    }
    let region = region_at(su);
    let p = look.place.xy + relative.xy;
    let w = max(length(dpdx(relative)), length(dpdy(relative)));
    let along_w = max(abs(dpdx(along)), abs(dpdy(along))) * SLEEPER_SPACING_M;
    var albedo = vec3f(0.5);
    var share = 0.0;
    switch mat {
        case MAT_PAINT: {
            let across = abs(road.x);
            let paint = road_surface(region, code, max(road.y - across, 0.0), across, road.y, along, p, w, w, along_w);
            albedo = paint.rgb;
            share = paint.a;
        }
        case MAT_SHOULDER: {
            albedo = road_surface(region, 3u, 1.0, 0.0, 1.0, 0.0, p, w, w, along_w).rgb;
        }
        case MAT_KERB: {
            albedo = vec3f(0.585, 0.567, 0.521) * (0.92 + 0.08 * noise(p / 0.5));
        }
        case MAT_SIDEWALK: {
            albedo = paving(region, p, w);
        }
        case MAT_BALLAST: {
            albedo = vec3f(0.30, 0.29, 0.27) * (0.85 + 0.3 * noise(p / 0.12));
        }
        default: {
            albedo = colour.rgb * colour.rgb;
            if (mat >= MAT_JUNCTION_FIRST && mat <= MAT_JUNCTION_LAST) {
                let j = junction_surface(mat, region, code, road, along, p, w, along_w);
                albedo = j.rgb;
                share = j.a;
            }
        }
    }
    let lit = light_up(albedo, n, sun_share(sun_at(su), relative, n, LIFT_SOLID), 1.0, look.horizon.w);
    var out = atmosphere(lit, relative, altitude);
    // A dirt or grass track shows the ground between its ruts.
    if (share > 0.0) {
        out = mix(out, ground_colour(su, relative, altitude, level), share);
    }
    return vec4f(out, 1.0);
}
