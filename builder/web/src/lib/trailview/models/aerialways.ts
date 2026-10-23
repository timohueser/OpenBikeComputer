// Aerial cableways in metres, z up, the cable along +y: pylons, stations and the carriers that hang from
// the cable, ported from the prototype's src/models/aerialways.ts. One seed per line gives its pylons,
// stations and carriers the same steel and livery; a gondola line takes a seed per cabin, so its cabins
// come in many colours.
import { srgb, Model, outline, pick, roof, shade, type Vec } from './flat';

export type Rig = 'cable_car' | 'gondola' | 'chair_lift' | 'drag_lift';

/** Each cable's distance from the line's middle. A T-bar's riders pass beside the mast, so its cables stand wider apart than its sheaves need. */
export const TRACK_HALF_M: Record<Rig, number> = { cable_car: 4, gondola: 2.7, chair_lift: 2.2, drag_lift: 1.2 };
/** A station's cables leave at x = ±TRACK_HALF_M, y = 0, this high over its ground, along +y. */
export const CABLE_AT_STATION_M: Record<Rig, number> = { cable_car: 8.4, gondola: 6, chair_lift: 4.2, drag_lift: 4.5 };
/** Footings reach this far below the ground at the anchor, so a slope does not show under them. */
const FOOTING_M = 3;

/** Galvanised, reseda green and blue-grey paint. */
const STEEL = ['#a3aaab', '#86a08c', '#8c979f'].map(srgb);
const LIVERY = ['#d0402f', '#2f6db0', '#e8b530', '#3f9a5c', '#e5702d'].map(srgb);
const TRAM = ['#c8372d', '#2f5f9e'].map(srgb);
const DARK = srgb('#30363a'), CONCRETE = srgb('#b8b2a6'), RENDER = srgb('#e6dfcf'), GLASS = srgb('#3b4c58');
/** A hall's big panes, lighter than a cabin's windows: they hold the sky. */
const PANES = srgb('#7790a0');
const WHITE = srgb('#eeece6'), TIMBER = srgb('#8f6440'), SLATE = srgb('#5a5652');

/** A flat bar of width w from a to b, facing `out`: the angle iron in a tower's face. */
function strip(m: Model, a: Vec, b: Vec, w: number, out: Vec, colour: number): void {
    const d: Vec = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    const c: Vec = [d[1] * out[2] - d[2] * out[1], d[2] * out[0] - d[0] * out[2], d[0] * out[1] - d[1] * out[0]], l = Math.hypot(...c) / (w / 2);
    const s = (p: Vec, k: number): Vec => [p[0] + (k * c[0]) / l, p[1] + (k * c[1]) / l, p[2] + (k * c[2]) / l];
    m.poly([s(a, -1), s(a, 1), s(b, 1), s(b, -1)], colour);
}

/** A profile in the y–z plane drawn `width` across x. */
const side = (m: Model, profile: [number, number][], width: number, colour: number) => m.add(new Model().prism(profile, -width / 2, width / 2, colour).place(Math.PI / 2));

/** A cable car's tapering lattice tower: four legs, X-braced faces in panels that shorten up the tower, and a broad cross-arm with a rope saddle at each end. */
function lattice(m: Model, height: number, steel: number): void {
    const T = TRACK_HALF_M.cable_car, head = height - 0.9, top = [1.1, 0.8], base = [top[0] + 0.065 * head, top[1] + 0.05 * head];
    const n = Math.min(8, Math.max(3, Math.round(head / 6))), q = (top[0] / base[0]) ** (1 / n);
    const zs = Array.from({ length: n + 1 }, (_, i) => ((1 - q ** i) / (1 - q ** n)) * head);
    const SQUARE = [[1, 1], [-1, 1], [-1, -1], [1, -1]];
    const corner = (z: number, k: number): Vec => {
        const t = z / head, [sx, sy] = SQUARE[k % 4];
        return [sx * (base[0] + (top[0] - base[0]) * t), sy * (base[1] + (top[1] - base[1]) * t), z];
    };
    for (let k = 0; k < 4; k++) {
        const [x, y] = corner(0, k), [a, b] = [SQUARE[k], SQUARE[(k + 1) % 4]], l = Math.hypot(a[0] + b[0], a[1] + b[1]);
        const out: Vec = [(a[0] + b[0]) / l, (a[1] + b[1]) / l, 0];
        m.beam(corner(-FOOTING_M, k), corner(head, k), 0.42, steel).box([x, y, 0.25 - FOOTING_M / 2], [1.1, 1.1, 0.9 + FOOTING_M], CONCRETE);
        for (let i = 0; i < n; i++) {
            strip(m, corner(zs[i], k), corner(zs[i + 1], k + 1), 0.2, out, steel);
            strip(m, corner(zs[i], k + 1), corner(zs[i + 1], k), 0.2, out, steel);
        }
    }
    const end = T + 0.3, root = top[0] + 0.2;
    m.box([0, 0, head - 0.4], [2 * top[0] + 0.3, 2 * top[1] + 0.3, 0.8], shade(steel, 0.8));
    m.prism([[-end, height - 0.9], [-root, height - 2.6], [root, height - 2.6], [end, height - 0.9], [end, height - 0.4], [-end, height - 0.4]], -0.45, 0.45, steel);
    for (const s of [-1, 1]) m.box([s * T, 0, height - 0.2], [0.55, 2.8, 0.4], DARK);
}

/** Mast radius at the foot and head, sheaves per side and their radius. */
const MAST: Record<Exclude<Rig, 'cable_car'>, { r: [number, number]; wheels: number; wheel: number }> = {
    gondola: { r: [0.55, 0.38], wheels: 4, wheel: 0.26 },
    chair_lift: { r: [0.42, 0.3], wheels: 4, wheel: 0.21 },
    drag_lift: { r: [0.24, 0.17], wheels: 2, wheel: 0.16 },
};

/** A tubular mast with a T cross-arm, and at each end a sheave train under the cable. The arm stops inside the tracks: the hangers bend out past the sheaves. */
function mast(m: Model, rig: Exclude<Rig, 'cable_car'>, height: number, steel: number): void {
    const T = TRACK_HALF_M[rig], { r: [r0, r1], wheels, wheel } = MAST[rig], axle = height - wheel, arm = axle - wheel - 0.3, dark = shade(steel, 0.8);
    m.box([0, 0, 0.15 - FOOTING_M / 2], [4 * r0 + 0.5, 4 * r0 + 0.5, 0.7 + FOOTING_M], CONCRETE);
    m.beam([0, 0, 0], [0, 0, arm], 2 * r0, steel, { sides: 8, w1: 2 * r1 });
    m.box([0, 0, arm], [2.4 * r1, 2.4 * r1, 0.8], dark).box([0, 0, arm], [2 * (T - 0.14), 1.6 * r1, 0.4], steel);
    const pitch = 2 * wheel + 0.08;
    for (const s of [-1, 1]) {
        const x = s * (T - 0.15);
        m.box([x, 0, axle], [0.14, wheels * pitch, 0.18], dark).box([x, 0, (arm + axle) / 2], [0.16, 0.22, axle - arm], dark);
        for (let k = 0; k < wheels; k++) {
            const y = (k - (wheels - 1) / 2) * pitch;
            m.beam([s * T - 0.07, y, axle], [s * T + 0.07, y, axle], 2 * wheel, DARK, { sides: 6, capped: true });
        }
    }
}

/** A pylon with its base at the origin; the cables run along +y over its top at z = height and x = ±TRACK_HALF_M. */
export function pylon(rig: Rig, height: number, seed = 1): Model {
    const m = new Model(), steel = pick(STEEL, seed);
    if (rig === 'cable_car') lattice(m, height, steel);
    else mast(m, rig, height, steel);
    return m;
}

/** An operator's hut on a concrete plinth: timber walls, a band of windows and a gable roof. */
function hut(m: Model, [x, y]: [number, number], livery: number): void {
    const [w, l, h] = [2.6, 3.2, 2.7];
    m.box([x, y, (0.3 - FOOTING_M) / 2], [w + 0.3, l + 0.3, 0.3 + FOOTING_M], CONCRETE).box([x, y, 0.3 + h / 2], [w, l, h], TIMBER).box([x, y, 2.05], [w + 0.08, l + 0.08, 0.9], GLASS);
    roof(m, [x, y, 0.3 + h], w, l, 30, 0.45, livery, TIMBER);
}

/**
 * A cable car's terminal: a rendered hall on a concrete storey, its roof in the line's livery rising
 * toward the line. At +y it opens a storey up in a bay over a platform at the cabin's floor; glazing
 * fills the high front over the bay.
 */
function tramStation(m: Model, livery: number): void {
    const T = TRACK_HALF_M.cable_car, cable = CABLE_AT_STATION_M.cable_car, [hx, hy, back, nose] = [T + 3, 8, 1, 2];
    const [floor, bay, low, high] = [cable - 5.4, cable + 0.8, cable, cable + 3.6], [depth, mid] = [hy - back, (hy + back) / 2];
    const rise = (high - low) / (2 * hy), eave = (y: number) => low + (y + hy) * rise, dim = shade(RENDER, 0.35), [o, front, t] = [0.8, 1.8, 0.45];
    m.box([0, 0, (floor - FOOTING_M) / 2], [2 * hx + 0.3, 2 * hy + 0.3, floor + FOOTING_M], CONCRETE);
    m.box([0, hy + nose / 2, (floor - FOOTING_M) / 2], [2 * T + 3.4, nose, floor + FOOTING_M], CONCRETE);
    side(m, [[-hy, floor], [back, floor], [back, eave(back)], [-hy, low]], 2 * hx, RENDER);
    m.box([0, back + 0.03, (floor + bay) / 2], [2 * hx - 1.4, 0.06, bay - floor], dim);
    m.box([-hx + 0.35, mid, (floor + bay) / 2], [0.7, depth, bay - floor], RENDER).box([hx - 0.35, mid, (floor + bay) / 2], [0.7, depth, bay - floor], RENDER);
    side(m, [[back, bay], [hy, bay], [hy, high], [back, eave(back)]], 2 * hx, RENDER);
    m.box([0, mid, bay - 0.05], [2 * hx - 1.4, depth, 0.1], dim).box([0, hy + 0.04, (bay + high) / 2 - 0.25], [2 * hx - 1, 0.1, high - bay - 1.5], PANES);
    for (const s of [-1, 1]) m.box([s * (hx + 0.03), -1, floor + 2.6], [0.1, 2 * hy - 5, 1.4], GLASS);
    side(m, [[-hy - 0.1, low - 0.8], [hy + 0.1, high - 0.8], [hy + 0.1, high], [-hy - 0.1, low]], 2 * hx + 0.2, SLATE);
    side(m, [[-hy - o, low - o * rise], [hy + front, high + front * rise], [hy + front, high + front * rise + t], [-hy - o, low - o * rise + t]], 2 * hx + 1.2, livery);
}

/** A gondola terminal: a concrete base up to the boarding floor, a glazed hall on corner columns under a broad roof in the line's livery, open at +y. */
function gondolaStation(m: Model, livery: number, steel: number): void {
    const T = TRACK_HALF_M.gondola, cable = CABLE_AT_STATION_M.gondola, [hx, hy, floor, eave, bay] = [T + 2, 7, cable - 3.8, cable + 1, 2];
    const [wx, y0, y1] = [hx - 0.2, 0.2 - hy, hy - 0.2 - bay];
    m.box([0, 0, (floor - FOOTING_M) / 2], [2 * hx, 2 * hy, floor + FOOTING_M], CONCRETE);
    m.box([0, (y0 + y1) / 2, (floor + eave) / 2], [2 * wx, y1 - y0, eave - floor], [PANES, PANES, shade(CONCRETE, 0.5), PANES, PANES, PANES]);
    roof(m, [0, 0, eave], 2 * hx, 2 * hy, 14, 0.9, livery, shade(livery, 0.8));
    for (const sx of [-1, 1]) {
        for (const sy of [-1, 1]) m.box([sx * (hx - 0.25), sy * (hy - 0.25), (floor + eave) / 2], [0.45, 0.45, eave - floor], steel);
        for (let i = 1; i < 5; i++) {
            const y = y0 + ((y1 - y0) * i) / 5;
            m.box([sx * (wx + 0.05), y, (floor + eave) / 2], [0.06, 0.16, eave - floor], steel);
        }
    }
}

/**
 * A chair lift's or T-bar's terminal: the cable turns round a bullwheel lying flat under a narrow hood
 * over the hub and drive. An A-frame behind the wheel carries the hood; the operator's hut stands beside.
 */
function liftStation(m: Model, rig: 'chair_lift' | 'drag_lift', livery: number, steel: number): void {
    const T = TRACK_HALF_M[rig], z = CABLE_AT_STATION_M[rig], chairs = rig === 'chair_lift';
    const rear = -(T + (chairs ? 1.25 : 0.55) + 0.6), [hx, y0, y1, spread] = chairs ? [1.4, rear - 0.4, 1.2, 1.9] : [0.85, rear - 0.4, 0.75, 1.3];
    m.add(new Model().loft(outline(hx, (y1 - y0) / 2, 0.6 * hx, 2), [[z + 0.3, 1], [z + 0.45, 1], [z + 1.0, 1], [z + 1.2, 0.86]], band => (band === 0 ? WHITE : livery), shade(livery, 0.5), livery)
        .map(([x, y, h]) => [x, y + (y0 + y1) / 2, h]));
    for (let k = 0; k < 16; k++) {
        const a = (k / 16) * 2 * Math.PI, b = ((k + 1) / 16) * 2 * Math.PI;
        m.beam([T * Math.cos(a), T * Math.sin(a), z], [T * Math.cos(b), T * Math.sin(b), z], 0.3, DARK);
    }
    m.beam([0, 0, z - 0.3], [0, 0, z + 0.4], 0.7, DARK, { sides: 8, capped: true });
    for (const k of [1, 3, 5, 7]) {
        const c = Math.cos((k * Math.PI) / 4), s = Math.sin((k * Math.PI) / 4);
        m.beam([0.3 * c, 0.3 * s, z], [(T - 0.1) * c, (T - 0.1) * s, z], 0.14, DARK);
    }
    const legAt = (h: number) => spread * (1 - (0.45 * (h + FOOTING_M)) / (z + FOOTING_M));
    m.box([0, rear, z + 0.1], [2 * legAt(z) + 0.6, 0.6, 0.5], steel);
    hut(m, [T + (chairs ? 3.2 : 2.4), 1.2], livery);
    for (const s of [-1, 1]) {
        m.beam([s * spread, rear, -FOOTING_M], [s * legAt(z), rear, z], 0.42, steel, { capped: true });
        m.box([s * spread, rear, (0.35 - FOOTING_M) / 2], [0.9, 0.9, 0.35 + FOOTING_M], CONCRETE);
    }
}

/** A station with its origin at the ground in its middle; its cables leave at x = ±TRACK_HALF_M, y = 0, z = CABLE_AT_STATION_M along +y. */
export function station(rig: Rig, seed = 1): Model {
    const m = new Model(), livery = pick(rig === 'cable_car' ? TRAM : LIVERY, seed, 1), steel = pick(STEEL, seed);
    if (rig === 'cable_car') tramStation(m, livery);
    else if (rig === 'gondola') gondolaStation(m, livery, steel);
    else liftStation(m, rig, livery, steel);
    return m;
}

/** How far below the cable a chair's seat is. */
const CHAIR_SEAT_M = 3.6;

/** A large cabin, 6 × 3 × 3 m, the long side along the cable: a tapered skirt, big windows, a white roof and a C-shaped hanger. */
function tram(m: Model, seed: number): void {
    const body = pick(TRAM, seed, 1), steel = pick(STEEL, seed);
    const glazed = (n: [number, number]) => Math.abs(n[0]) > 0.99 || Math.abs(n[1]) > 0.99;
    m.loft(outline(1.5, 3, 0.55, 1), [[-5.3, 0.86], [-4.85, 1], [-4.1, 1], [-2.95, 1.04], [-2.55, 1.04], [-2.35, 0.97]],
        (band, n) => [shade(body, 0.72), body, glazed(n) ? GLASS : body, body, WHITE][band], DARK, WHITE);
    m.box([0, 0, 0.15], [0.7, 3.6, 0.6], shade(steel, 0.55));
    m.beam([0.1, 0, -0.05], [0.95, 0, -0.6], 0.5, steel, { capped: true }).beam([0.95, 0, -0.5], [0.95, 0, -1.85], 0.5, steel, { capped: true });
    m.beam([0.95, 0, -1.75], [0, 0, -2.4], 0.5, steel, { capped: true });
}

/** A rounded 8-seat cabin, 1.8 × 2.2 × 2.2 m, windows all round between coloured corners. */
function gondola(m: Model, seed: number): void {
    const body = pick(LIVERY, seed, 1), steel = pick(STEEL, seed);
    m.loft(outline(0.9, 1.1, 0.4, 2), [[-3.7, 0.8], [-3.4, 1], [-2.8, 1], [-1.8, 1.03], [-1.65, 1.02], [-1.5, 0.85]],
        (band, n) => [shade(body, 0.72), body, Math.abs(n[0]) > 0.99 || Math.abs(n[1]) > 0.99 ? GLASS : body, body, WHITE][band], DARK, WHITE);
    m.box([0, 0, 0], [0.24, 0.5, 0.3], DARK);
    const hanger: Vec[] = [[0.04, 0, -0.06], [0.42, 0, -0.28], [0.56, 0, -0.8], [0.34, 0, -1.35], [0, 0, -1.53]];
    hanger.slice(1).forEach((p, i) => m.beam(hanger[i], p, 0.11, steel));
}

/** A 4-seat chair facing +y with seat and backrest, its safety bar down over a footrest. */
function chair(m: Model, seed: number): void {
    const pad = pick(LIVERY, seed, 1), steel = pick(STEEL, seed), [top, back, seat, half] = [-CHAIR_SEAT_M + 0.65, -0.32, -CHAIR_SEAT_M, 1.22];
    m.box([0, 0, 0], [0.2, 0.45, 0.26], DARK);
    const hanger: Vec[] = [[0.04, 0, -0.06], [0.4, 0, -0.3], [0.48, 0, -1.1], [0.38, 0, top + 0.7], [0.1, back * 0.6, top + 0.15], [0, back, top]];
    hanger.slice(1).forEach((p, i) => m.beam(hanger[i], p, 0.1, steel));
    m.beam([-half, back, top], [half, back, top], 0.08, steel);
    m.box([0, back + 0.06, top - 0.33], [2 * half - 0.06, 0.14, 0.55], pad).box([0, -0.02, seat], [2 * half - 0.06, 0.56, 0.16], pad);
    for (const s of [-1, 1]) m.beam([s * half, back, top], [s * half, 0.28, seat - 0.05], 0.06, steel).beam([s * half, back, top], [s * half, 0.55, top - 0.12], 0.05, steel);
    m.beam([-half, 0.55, top - 0.12], [half, 0.55, top - 0.12], 0.05, steel).beam([0, 0.55, top - 0.12], [0, 0.62, seat - 0.4], 0.05, steel);
    m.beam([-0.8, 0.62, seat - 0.4], [0.8, 0.62, seat - 0.4], 0.06, steel);
}

/** A T-bar wound up under its spring box, as it hangs between riders. */
function tbar(m: Model, seed: number): void {
    const housing = pick(LIVERY, seed, 1), steel = pick(STEEL, seed);
    m.box([0, 0, 0], [0.2, 0.4, 0.26], DARK).beam([0.05, 0, -0.05], [0.32, 0, -0.25], 0.06, DARK).beam([0.32, 0, -0.2], [0.06, 0, -0.62], 0.06, DARK);
    m.box([0, 0, -0.9], [0.32, 0.42, 0.6], housing).beam([0, 0, -1.2], [0, 0, -2.9], 0.06, steel).beam([-0.5, 0, -2.9], [0.5, 0, -2.9], 0.07, steel, { capped: true });
}

/** A carrier hanging from its grip at the origin, the cable along y. Hangers bend out to +x past the sheaves: turn a carrier half round for the track at −x. */
export function carrier(rig: Rig, seed = 1): Model {
    const m = new Model();
    ({ cable_car: tram, gondola, chair_lift: chair, drag_lift: tbar })[rig](m, seed);
    return m;
}
