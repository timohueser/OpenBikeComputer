// The looks of what tracks.ts and parts.ts stand on a way: the treads and risers of steps in a town, the logs
// of steps on a trail, and a track's sleepers, rails, rack and cable.
// #include looks.wgsl

// Near the eye these faces draw over the way. From DETAIL_M to DETAIL_END_M they discard a growing share of their
// pixels, over a way that paints them in full, so the two add up to one. DETAIL_END_M as roads.ts, which culls them past it.
const DETAIL_M: f32 = 140.0;
const DETAIL_END_M: f32 = 190.0;
// As parts.ts: the middle of a rail from a track's middle, and half a sleeper, of each gauge.
const RAIL_GAUGE_M: f32 = 0.7525;
const NARROW_GAUGE_M: f32 = 0.53;
const SLEEPER_HALF_M: f32 = 1.3;
const NARROW_SLEEPER_HALF_M: f32 = 0.9;
const LOOK_TREAD: u32 = 31u;
const LOOK_RISER: u32 = 32u;
const LOOK_SLEEPER: u32 = 33u;
const LOOK_TIMBER: u32 = 34u;
const LOOK_RAIL: u32 = 35u;
const LOOK_RAIL_HEAD: u32 = 36u;
const LOOK_RACK: u32 = 37u;
const LOOK_CABLE: u32 = 38u;
const LOOK_LOG: u32 = 39u;

// A log is a hand high, and the sunlit soil around it lights its side out of the sun: it lights by a normal that
// leans this far up, or that side keeps only the cool sky and reads as slate.
const LOG_LEAN: f32 = 0.7;

// A log lit, in the haze. The shadow map holds nothing true of a face turned from the sun, so only the terrain's
// sun shades it.
fn log_lit(l: WayLook, n: vec3f, su: vec2f, relative: vec3f, altitude: f32) -> vec3f {
    return atmosphere(light_up(l.albedo, normalize(n + vec3f(0.0, 0.0, LOG_LEAN)), sun_at(su), 1.0, look.horizon.w), relative, altitude);
}

// The share of its pixels that a face discards `distance` metres off.
fn detail_fade(distance: f32) -> f32 {
    return smoothstep(DETAIL_M, DETAIL_END_M, distance);
}

// Whether a way paints these faces `distance` metres off: nearer, they hide all that it could paint.
fn detail_painted(distance: f32) -> bool {
    return distance >= DETAIL_M;
}

fn narrow_gauge(code: u32) -> bool {
    return ((code - RAIL_CODE) & 1u) != 0u;
}

fn rail_gauge(code: u32) -> f32 {
    return select(RAIL_GAUGE_M, NARROW_GAUGE_M, narrow_gauge(code));
}

fn sleeper_half(code: u32) -> f32 {
    return select(SLEEPER_HALF_M, NARROW_SLEEPER_HALF_M, narrow_gauge(code));
}

// Steps in a town: concrete or stone treads, each its own tone with a lighter worn middle and a nosing. A
// tread's `along` is metres back from its nosing.
fn step_look(mat: u32, w: WayPixel) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let p = w.p;
    let h = hash(floor(p / 0.9) + floor(w.along * 3.0));
    var c = vec3f(0.36, 0.355, 0.34) * (0.9 + 0.15 * h) * (0.93 + 0.1 * noise(p / 0.25)) * (1.0 + 0.15 * seen(0.02, w.pw) * (noise(p / 0.008) - 0.5));
    if (mat == LOOK_RISER) {
        c *= 0.85;
    } else {
        c *= 1.0 + 0.08 * smoothstep(0.5, 0.0, abs(w.across));
        // The nosing: a lighter worn edge, a dark line where it meets the riser below.
        c *= 1.0 + 0.12 * band(w.along, 0.0, 0.05, w.aw) - 0.4 * band(w.along, -1.0, 0.008, w.aw);
    }
    out.albedo = c;
    return out;
}

// Concrete sleepers, the dark pads and clips of the rails on their top; `up` is the face's normal's z.
fn sleeper_look(code: u32, w: WayPixel, up: f32) -> vec3f {
    let gauge = rail_gauge(code);
    let x = abs(w.across);
    let c = vec3f(0.2, 0.195, 0.185) * (0.9 + 0.15 * noise(w.p / 0.2)) * mix(vec3f(1.0), vec3f(1.1, 0.92, 0.8), smoothstep(0.35, 0.0, abs(x - gauge))) * (1.0 + 0.1 * seen(0.02, w.pw) * (noise(w.p / 0.01) - 0.5));
    return mix(c, vec3f(0.05, 0.045, 0.04), band(x, gauge - 0.11, gauge + 0.11, w.cw) * step(0.8, up));
}

// A ladder rack: rungs 0.1 m apart on its top; dark steel at its sides.
fn rack_look(w: WayPixel, up: f32) -> vec3f {
    let rung = band(fract(w.along * SLEEPER_SPACING_M / 0.1 + 0.5) - 0.5, -0.25, 0.25, w.aw * SLEEPER_SPACING_M / 0.1);
    return mix(vec3f(0.035, 0.03, 0.028), vec3f(0.2, 0.19, 0.18), select(0.0, rung, up > 0.8));
}

// The look of a face of tracks.ts or parts.ts.
fn track_look(mat: u32, code: u32, w: WayPixel, up: f32) -> WayLook {
    switch mat {
        case LOOK_SLEEPER: {
            return WayLook(sleeper_look(code, w, up), vec2f(0.0), 0.0, 0.0, 0.0);
        }
        case LOOK_TIMBER: {
            // Dark with creosote, its grain across the track.
            return WayLook(vec3f(0.09, 0.065, 0.045) * (0.8 + 0.4 * noise(vec2f(w.across / 0.5, (w.p.x + w.p.y) / 0.01))), vec2f(0.0), 0.0, 0.0, 0.0);
        }
        case LOOK_RAIL: {
            return WayLook(vec3f(0.13, 0.075, 0.045) * (0.8 + 0.3 * noise(w.p / 0.3)), vec2f(0.0), 0.0, 0.1, 0.0);
        }
        case LOOK_RAIL_HEAD: {
            return WayLook(vec3f(0.42, 0.43, 0.44), vec2f(0.0), 0.0, 1.6, 0.0);
        }
        case LOOK_RACK: {
            return WayLook(rack_look(w, up), vec2f(0.0), 0.0, 0.15, 0.0);
        }
        case LOOK_CABLE: {
            return WayLook(vec3f(0.06, 0.06, 0.065), vec2f(0.0), 0.0, 0.5, 0.0);
        }
        case LOOK_LOG: {
            // Weathered timber, grey over brown, with dark cracks along its grain. Its `along` is metres ahead of its axis:
            // it darkens towards the ground, so its few facets read round.
            let grain = noise(vec2f(w.across / 0.5, w.along / 0.012));
            let crack = smoothstep(0.62, 0.7, noise(vec2f(w.across / 0.25, w.along / 0.005))) * seen(0.01, w.pw);
            let round = mix(1.0, 0.6, smoothstep(0.03, 0.08, abs(w.along)));
            return WayLook(vec3f(0.27, 0.245, 0.21) * (0.8 + 0.35 * grain) * (1.0 - 0.6 * crack) * round, vec2f(0.0), 0.0, 0.0, 0.0);
        }
        default: {
            return step_look(mat, w);
        }
    }
}
