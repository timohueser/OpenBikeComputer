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
    /** The bank's rise per metre, the wall's height (0: none) and thickness, and whether the bank runs round the end behind the portal line. */
    slope: number; wall: number; thick: number; cap: boolean;
    /** Whether a railing stands on the walls and on the end face. */
    railing?: boolean;
}

const unit = ([x, y]: [number, number]): [number, number] => [x / Math.hypot(x, y), y / Math.hypot(x, y)];

/** A road's mouth: variant A has walls of at most 4.5 m beside the cut, variant B the slope cut back at 2:3. */
const road = (id: string, o: [number, number], d: [number, number], m: number, z0: number, zL: number, L: number, s0: number, gl: number, gr: number): Mouth => ({
    id, o, d: unit(d), m, z0, grade: (zL - z0) / L, before: 0, s0, L, back: 20, h: 6, fh: 7.3, oh: 6.6, keep: 0, hh: 5, H: 6.3, chamfer: 0.9,
    w: 24, gl, gr, slope: 2 / 3, wall: VARIANT === 'A' ? 4.5 : 0, thick: VARIANT === 'A' ? 0.4 : 0, cap: VARIANT !== 'B2',
});

/** The mouths of the build's variant. Heights set by hand are marked. */
export const MOUTHS: Mouth[] = !VARIANT ? [] : [
    // T02, the two tubes of the A14 at Schönegg: the floors are the bake's (the tunnel line); each portal line is the first
    // station where the plain ground lies 6.3 m (bore 5.0, cover 1.0, roof 0.3) over the floor at both edges of the frame.
    road('T02a', [562347898, 377452914], [-318, -435], 0.025402708060149663, 524.17, 523.525, 15, -2.5, 10, 26),
    road('T02b', [562347310, 377453358], [-299, -397], 0.025402708060149663, 524.41, 523.89, 12, -2.5, 26, 10),
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
export const floor = (k: Mouth, s: number) => k.z0 + (s < 0 ? k.before * s : k.grade * Math.min(s, k.L));

/** The zones of a mouth's cut: the side beside the floor before the portal line, and behind it the ground over the bore and beside it. */
export type Zone = 'side' | 'over' | 'cone';

/** The cut's height in a zone of a mouth, whatever the point: the mesh reads each face in its own zone. */
export function zoneField(k: Mouth, zone: Zone, s: number, a: number): number {
    const side = Math.max(Math.abs(a) - k.fh - k.thick, 0), t = Math.max(s - k.L, 0);
    if (zone === 'side') return floor(k, s) + k.wall + k.slope * side;
    if (zone === 'over') return floor(k, k.L) + k.H + k.slope * t;
    return floor(k, k.L) + k.wall + k.slope * Math.hypot(side, t);
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
export const mouthAt = (x: number, y: number) => MOUTHS.find(k => Math.hypot(x - k.o[0], y - k.o[1]) < 4);

/** Whether a line's first or last vertex is a mouth's tag end. */
export const isProto = (xy: number[]) => !!(mouthAt(xy[0], xy[1]) ?? mouthAt(xy[xy.length - 2], xy[xy.length - 1]));

const f = (v: number) => (Number.isInteger(v) ? `${v}.0` : `${v}`);

/**
 * The cut field in WGSL, for the terrain, the plan's pieces and the structures: `proto_field(u)` at `u` units about the
 * anchor, PROTO_INF where nothing is cut (`proto_cut`). Empty for today's build.
 */
export function protoWgsl(): string {
    if (!MOUTHS.length) return '';
    const call = (k: Mouth) => `proto_mouth(proto_local(u, vec2i(${k.o[0]}, ${k.o[1]}), vec2f(${k.d[0]}, ${k.d[1]}), ${k.m}), ${[k.z0, k.grade, k.before, k.s0, k.L, k.back, k.fh, k.keep, k.H, k.w, k.slope, k.wall, k.thick, k.cap ? 1 : 0].map(f).join(', ')})`;
    return `
const PROTO_INF: f32 = 1e9;
fn proto_local(u: vec2f, o: vec2i, d: vec2f, m: f32) -> vec2f {
    let p = (u - vec2f(o - surface.anchor)) * m;
    return vec2f(dot(p, d), d.x * p.y - d.y * p.x);
}
// x: the cut's height of one mouth; y: 1 where the plan keeps its road, 2 on the cut's floor, 3 over the bore behind the portal line.
fn proto_mouth(q: vec2f, z0: f32, grade: f32, before: f32, s0: f32, portal: f32, back: f32, fh: f32, keep: f32, top: f32, w: f32, slope: f32, wall: f32, thick: f32, cap: f32) -> vec2f {
    if (q.x < s0 || q.x > portal + back || abs(q.y) > w) {
        return vec2f(PROTO_INF, 0.0);
    }
    let side = max(abs(q.y) - fh - thick, 0.0);
    let inside = abs(q.y) <= fh;
    if (q.x < 0.0 && abs(q.y) < keep) {
        return select(vec2f(z0 + ${f(END_OVER_M)}, 0.0), vec2f(PROTO_INF, 1.0), q.x < -${f(END_M)});
    }
    let zf = z0 + select(grade * min(q.x, portal), before * q.x, q.x < 0.0);
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
