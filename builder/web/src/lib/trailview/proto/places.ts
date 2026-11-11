// PROTOTYPE (crossing look study, never merged): the places, the variant of the build, and the cut field: the height of
// the ground that the prototype opens, as the mesh (proto/mouth.ts) and the shaders (protoWgsl) read it.
import { UNITS } from '../frame/space';

/** The variant of the build: '' today, 'A' or 'B'; 'B2' is B without the bank round the end of a road's mouth. */
export const VARIANT = ((import.meta as unknown as { env?: Record<string, string> }).env?.VITE_PROTO ?? '') as '' | 'A' | 'B' | 'B2';

export const world = (lat: number, lon: number): [number, number] => [(lon + 180) / 360 * UNITS, (1 - Math.asinh(Math.tan(lat * Math.PI / 180)) / Math.PI) / 2 * UNITS];

/** The prototype places: id, lat, lon. */
export const PLACES: [string, number, number][] = [['T02', 47.0480522, 8.5418249], ['S1', 46.95829, 8.38676], ['T14', 46.822618, 8.401126]];

/**
 * A mouth: the open cut of a tunnel way from before its tag end to the portal line, and the frame there. Local metres:
 * `s` along the way into the tunnel from the tag end, `a` across, to the right of that way.
 */
export interface Mouth {
    id: string;
    /** The tag end in world units, the unit way into the tunnel in world axes (x east, y south), metres per unit. */
    o: [number, number]; d: [number, number]; m: number;
    /** The floor at the tag end; its rise per metre in from there to the portal line, and from the start of the cut to the tag end. */
    z0: number; grade: number; before: number;
    /** The start of the cut (≤ 0), the portal line, and the reach of the round cap behind it. */
    s0: number; L: number; back: number;
    /** The half-widths: the way's paved surface, the cut's floor (the frame's outer edge), the opening; the plan keeps its own road before the tag end to `keep`. */
    h: number; fh: number; oh: number; keep: number;
    /** The opening's height, the frame's top, and the cut of the opening's two upper corners. */
    hh: number; H: number; chamfer: number;
    /** The reach of the cut to each side, and of the mouth's own mesh to the left and to the right. */
    w: number; gl: number; gr: number;
    /** The bank's rise per metre; left and right of the way the wall's height (0: a bank) and thickness; whether the bank runs round the end behind the portal line. */
    slope: number; walls: [number, number]; thicks: [number, number]; cap: boolean;
    /** The tunnel line's end that the mouth belongs to, in world units. */
    tag: [number, number];
    /** A pit beside the way on top: the floor is level from station `flat`; the walls' top; the bore leaves through the side `dir` (-1 left) at station `at`, `inset` metres from the line's end, at the level `boreZ`. */
    flat?: number; top?: number; side?: { at: number; dir: 1 | -1 }; inset?: number; boreZ?: number;
    /** A pit's sides without a bank: concrete walls with a railing, or steep faces of earth. */
    earth?: boolean;
    /** Whether the floor beside the way's own surface wears the ground's look: a path's mouth. */
    soft?: boolean;
    /** The bore's floor set by hand: metres along the tunnel line from its start, and the level there. */
    profile?: [number, number][];
}

const unit = ([x, y]: [number, number]): [number, number] => [x / Math.hypot(x, y), y / Math.hypot(x, y)];

/** A road's mouth: variant A has walls of at most 4.5 m beside the cut, variant B the slope cut back at 2:3. */
const road = (id: string, o: [number, number], d: [number, number], m: number, z0: number, zL: number, L: number, s0: number, gl: number, gr: number): Mouth => ({
    id, o, d: unit(d), m, z0, grade: (zL - z0) / L, before: 0, s0, L, back: 20, h: 6, fh: 7.3, oh: 6.6, keep: 0, hh: 5, H: 6.3, chamfer: 0.9,
    w: 24, gl, gr, slope: 2 / 3, walls: VARIANT === 'A' ? [4.5, 4.5] : [0, 0], thicks: VARIANT === 'A' ? [0.4, 0.4] : [0, 0], cap: VARIANT !== 'B2', tag: o,
});

/** The floor of the footway tunnel at T14, set by hand: 0.5 m lower than the bake's under the service road, so that a bore of 2.2 m fits under it. */
const T14_FLOOR: [number, number][] = [[0, 1041.05], [11, 1043.3], [28, 1048.77]];
/** A path's mouth: an opening of 3.0 m by 2.2 m (a thin bore); variant A has walls beside the cut, variant B grass banks at 2:3. `before` is the floor's rise per metre before the tag end, as the ground lies there. */
const path = (id: string, o: [number, number], d: [number, number], m: number, z0: number, zL: number, L: number, before: number): Mouth => ({
    id, o, d: unit(d), m, z0, grade: (zL - z0) / L, before, s0: -1.5, L, back: 9, h: 0.6, fh: 1.8, oh: 1.5, keep: 0, hh: 2.2, H: 2.6, chamfer: 0,
    w: 10, gl: 11, gr: 11, slope: 2 / 3, walls: VARIANT === 'A' ? [4.5, 4.5] : [0, 0], thicks: VARIANT === 'A' ? [0.3, 0.3] : [0, 0], cap: true, tag: o, soft: true, profile: T14_FLOOR,
});

const S1_M = 0.025445425802497635, TALL = 30;
/**
 * A pit of the subway at S1, beside the road: from `tag`, a tunnel line's end, the way down runs `length` metres toward
 * `d` (east, north) to the bore's axis, which lies `shift` metres east of the tag. Variant A: a grass bank at 1:1 on the
 * side `free` (0: none has room) and a steep face of earth elsewhere; variant B: concrete walls with a railing.
 */
function pit(id: string, tag: [number, number], shift: number, d: [number, number], length: number, zTop: number, dir: 1 | -1, free: -1 | 0 | 1): Mouth {
    const u = unit(d), w: [number, number] = [u[0], -u[1]], axis: [number, number] = [tag[0] + shift / S1_M, tag[1]];
    const o: [number, number] = [Math.round(axis[0] - w[0] * length / S1_M), Math.round(axis[1] - w[1] * length / S1_M)], flat = length - 1.5, wall = (side: number) => (VARIANT === 'A' && side === free ? 0 : TALL);
    return {
        id, o, d: w, m: S1_M, z0: zTop, grade: (S1_FLOOR - zTop) / flat, before: 0, s0: -0.3, L: length + 1.9, back: 0, h: 1.5, fh: 1.5, oh: 1.5, keep: 0, hh: 2.5, H: 2.9, chamfer: 0,
        w: 9, gl: 8, gr: 8, slope: 1, walls: [wall(-1), wall(1)], thicks: VARIANT === 'B' ? [0.3, 0.3] : [0, 0], cap: false, tag,
        flat, top: 457.75, side: { at: length, dir }, inset: 1.5 - dir * shift, boreZ: S1_FLOOR, earth: VARIANT === 'A',
    };
}
/** The floor of the subway's box at S1: 2.4 m of room and a roof of 0.4 m under the road at 457.64. Set by hand. */
const S1_FLOOR = 454.85;

/** The mouths of the build's variant. Heights set by hand are marked. */
export const MOUTHS: Mouth[] = !VARIANT ? [] : [
    // T02, the two tubes of the A14 at Schönegg: the floors are the bake's (the tunnel line); each portal line is the first
    // station where the plain ground lies 6.3 m (bore 5.0, cover 1.0, roof 0.3) over the floor at both edges of the frame.
    road('T02a', [562347898, 377452914], [-318, -435], 0.025402708060149663, 524.17, 523.525, 15, -2.5, 10, 26),
    road('T02b', [562347310, 377453358], [-299, -397], 0.025402708060149663, 524.41, 523.89, 12, -2.5, 26, 10),
    // S1, the footway under the secondary road: the heights are set by hand (the bake has 1.4 m of room). East of the road the way
    // down is 12.5 m long, 3.5 m more than the mapped ramp; west of it 8.7 m, between the road and a house.
    pit('S1e', [561885656, 377845472], 0.3, [0.7, -9.0], 12.53, 457.6, 1, -1),
    pit('S1w', [561885116, 377845556], -0.3, [1.4, -8.6], 8.71, 457.55, -1, 0),
    // T14, the footway tunnel on the meadow: the floor is set by hand (T14_FLOOR).
    // Each portal line is where the plain ground lies 2.5 m (bore 2.2, roof 0.3) over the floor at both edges of the frame.
    path('T14a', [561928246, 378437626], [-48, -65], 0.025509873405052078, 1041.05, 1042.28, 6, 0.1),
    path('T14b', [561927678, 378436686], [44, 68], 0.025509873405052078, 1048.77, 1046.84, 6, -0.5),
];

const INF = 1e9;
/** Before a tag end the plan keeps its road; over the last metres only what stands this far over it, so that the plan's wall across the way's end goes. */
const END_M = 2.5, END_OVER_M = 0.35;

/** A world point in a mouth's local metres. */
export function local(k: Mouth, x: number, y: number): [number, number] {
    const px = (x - k.o[0]) * k.m, py = (y - k.o[1]) * k.m;
    return [px * k.d[0] + py * k.d[1], -k.d[1] * px + k.d[0] * py];
}

/** The world point of a mouth's local metres. */
export const place = (k: Mouth, s: number, a: number): [number, number] => [k.o[0] + (k.d[0] * s - k.d[1] * a) / k.m, k.o[1] + (k.d[1] * s + k.d[0] * a) / k.m];

/** The floor of a mouth at station s. */
export const floor = (k: Mouth, s: number) => k.z0 + (s < 0 ? k.before * s : k.grade * Math.min(s, k.flat ?? k.L));

/** The wall's height and thickness on the side of `a`. */
export const wallOf = (k: Mouth, a: number) => k.walls[a < 0 ? 0 : 1], thickOf = (k: Mouth, a: number) => k.thicks[a < 0 ? 0 : 1];

/** The zones of a mouth's cut: the side beside the floor before the portal line, and behind it the ground over the bore and beside it. */
export type Zone = 'side' | 'over' | 'cone';

/** The cut's height in a zone of a mouth, whatever the point: the mesh reads each face in its own zone. */
export function zoneField(k: Mouth, zone: Zone, s: number, a: number): number {
    const side = Math.max(Math.abs(a) - k.fh - thickOf(k, a), 0), t = Math.max(s - k.L, 0);
    if (zone === 'side') return floor(k, s) + wallOf(k, a) + k.slope * side;
    if (zone === 'over') return floor(k, k.L) + k.H + k.slope * t;
    return floor(k, k.L) + wallOf(k, a) + k.slope * Math.hypot(side, t);
}

/** The cut's height at a mouth's local point; INF where the mouth cuts nothing. As `proto_mouth` of protoWgsl. */
export function mouthField(k: Mouth, s: number, a: number): number {
    if (s < k.s0 || s > k.L + k.back || Math.abs(a) > k.w) return INF;
    const inside = Math.abs(a) <= k.fh;
    if (s < 0 && Math.abs(a) < k.keep) return s < -END_M ? INF : k.z0 + END_OVER_M;
    if (s <= k.L) return inside ? floor(k, s) : zoneField(k, 'side', s, a);
    if (!k.cap) return INF;
    return zoneField(k, inside ? 'over' : 'cone', s, a);
}

/** The cut's height at a world point: the lowest of the mouths', but for one's own. */
export function fieldAt(x: number, y: number, but?: Mouth): number {
    let f = INF;
    for (const k of MOUTHS) {
        const [s, a] = local(k, x, y);
        // The plan keeps its road before a tag end: no mouth cuts it.
        if (s >= k.s0 && s < -END_M && Math.abs(a) < k.keep) return INF;
        // The ground over a bore behind its portal line is its mouth's alone: no other mouth's bank cuts into the bore.
        if (k.cap && (k === but ? s > k.L - 1e-6 : s > k.L) && s <= k.L + k.back + 1 && Math.abs(a) <= k.fh + 1e-6) return k === but ? INF : zoneField(k, 'over', s, a);
        if (k !== but) f = Math.min(f, mouthField(k, s, a));
    }
    return f;
}

/** The mouth whose tag end is a line's vertex at world x, y. */
export const mouthAt = (x: number, y: number) => MOUTHS.find(k => Math.hypot(x - k.tag[0], y - k.tag[1]) < 4);

/** Whether a line's first or last vertex is a mouth's tag end. */
export const isProto = (xy: number[]) => !!(mouthAt(xy[0], xy[1]) ?? mouthAt(xy[xy.length - 2], xy[xy.length - 1]));

const f = (v: number) => (Number.isInteger(v) ? `${v}.0` : `${v}`);

/**
 * The cut field in WGSL, for the terrain, the plan's pieces and the structures: `proto_field(u)` at `u` units about the
 * anchor, PROTO_INF where nothing is cut (`proto_cut`). Empty for today's build.
 */
export function protoWgsl(): string {
    if (!MOUTHS.length) return '';
    const call = (k: Mouth) => `proto_mouth(proto_local(u, vec2i(${k.o[0]}, ${k.o[1]}), vec2f(${k.d[0]}, ${k.d[1]}), ${k.m}), ${[k.z0, k.grade, k.before, k.s0, k.L, k.flat ?? k.L, k.back, k.fh, k.keep, k.H, k.w, k.slope, ...k.walls, ...k.thicks, k.cap ? 1 : 0].map(f).join(', ')})`;
    return `
const PROTO_INF: f32 = 1e9;
fn proto_local(u: vec2f, o: vec2i, d: vec2f, m: f32) -> vec2f {
    let p = (u - vec2f(o - surface.anchor)) * m;
    return vec2f(dot(p, d), d.x * p.y - d.y * p.x);
}
// x: the cut's height of one mouth; y: 1 where the plan keeps its road, 2 on the cut's floor, 3 over the bore behind the portal line.
fn proto_mouth(q: vec2f, z0: f32, grade: f32, before: f32, s0: f32, portal: f32, level: f32, back: f32, fh: f32, keep: f32, top: f32, w: f32, slope: f32, wall_l: f32, wall_r: f32, thick_l: f32, thick_r: f32, cap: f32) -> vec2f {
    if (q.x < s0 || q.x > portal + back || abs(q.y) > w) {
        return vec2f(PROTO_INF, 0.0);
    }
    let wall = select(wall_r, wall_l, q.y < 0.0);
    let side = max(abs(q.y) - fh - select(thick_r, thick_l, q.y < 0.0), 0.0);
    let inside = abs(q.y) <= fh;
    if (q.x < 0.0 && abs(q.y) < keep) {
        return select(vec2f(z0 + ${f(END_OVER_M)}, 0.0), vec2f(PROTO_INF, 1.0), q.x < -${f(END_M)});
    }
    let zf = z0 + select(grade * min(q.x, level), before * q.x, q.x < 0.0);
    if (q.x <= portal) {
        return vec2f(zf + select(wall + slope * side, 0.0, inside), select(0.0, 2.0, inside));
    }
    if (cap < 0.5) {
        return vec2f(PROTO_INF, 0.0);
    }
    let t = q.x - portal;
    return vec2f(zf + select(wall + slope * length(vec2f(side, t)), top + slope * t, inside), select(0.0, 3.0, inside));
}
// x: the cut's height at u units about the anchor, PROTO_INF where nothing is cut; y: 2 on a cut's floor.
fn proto_cut(u: vec2f) -> vec2f {
    var f = vec2f(PROTO_INF, 0.0);
    var m = vec2f(0.0);
${MOUTHS.map(k => `    m = ${call(k)};\n    if (m.y == 1.0) { return vec2f(PROTO_INF, 0.0); }\n    if (m.y == 3.0) { return vec2f(m.x, 0.0); }\n    f = vec2f(min(f.x, m.x), max(f.y, m.y));`).join('\n')}
    return f;
}
`;
}

/** The lines that take the place of each \`// #proto NAME\` line of a shader, in a variant's build. */
export const PROTO_LINES: Record<string, string> = {
    // drawn-height.wgsl: the field's declarations.
    field: protoWgsl(),
    // terrain.wgsl: the terrain draws nothing over the cut.
    terrain: 'if (proto_cut(units_of(in.s)).x + 0.1 < in.h) { discard; }',
    // corridor.wgsl: the plan's pieces draw nothing over the cut, and nothing on its floor.
    piece: 'let proto = proto_cut(raw.u); if (proto.x + 0.1 < raw.altitude || proto.y == 2.0) { discard; }',
    // structures.wgsl, vertex: a prototype vertex stands at most `half` under the plain ground.
    vertex: 'if (v.sleepers < -1.5e9) { z = min(v.z, plain_height(s) - v.road.y * ROAD_RANGE_M); }',
    // structures.wgsl, fragment: a prototype ground face wears the ground's look with its own normal.
    fragment: `if (in.sleepers < -1.5e9 && in.sleepers > -2.5e9) {
        let su = lod_space(in.u);
        return vec4f(ground_colour(su, in.relative, mat2x3f(dpdx(in.relative), dpdy(in.relative)), in.altitude, u32(ceil(lod(su))), in.clip.xy, Road(vec3f(0.0), 0.0, 0.0, 0.0, 0.0, 0.0), vec4f(n, 0.05)), 1.0);
    }`,
};
