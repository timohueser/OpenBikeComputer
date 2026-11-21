// Prototype #3786 (never merges): the style candidates of the animals, built with the modelling kit's first cut. Each
// breed here is one model, 1 m tall at the withers' top, feet at the origin, z up, facing +y, as the breeds of animals.ts.
import { rgb } from '../structures/mesh';
import { Model, shade, type Vec } from './flat';

export interface Candidate { height: number; build: () => Model }

// ---- The kit: a skin through rings, a leaf, a chain. Every face turns away from a point inside its part. ----

/** A ring of a skin: its middle, its half-width, its reach above and below the middle, the pitch of its axis above +y in degrees, and how square it is (2 is an ellipse). */
interface Ring { at: Vec; w: number; up: number; down: number; pitch: number; e: number }
/** The colour of a face from its band (−1 and the band count are the end caps) and from how far up (−1 to 1) its facet looks. */
type Paint = (band: number, top: number) => number;

const sub = (a: Vec, b: Vec): Vec => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const along = (a: Vec, d: Vec, k: number): Vec => [a[0] + d[0] * k, a[1] + d[1] * k, a[2] + d[2] * k];
const cross = (a: Vec, b: Vec): Vec => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const unit = (a: Vec): Vec => along([0, 0, 0], a, 1 / (Math.hypot(...a) || 1));

/** A convex face whose normal points away from `inside`. */
function face(m: Model, corners: Vec[], colour: number, inside: Vec): Model {
    const n = cross(sub(corners[1], corners[0]), sub(corners[2], corners[0]));
    const mid = corners.reduce((sum, p) => along(sum, p, 1 / corners.length), [0, 0, 0] as Vec), out = sub(mid, inside);
    return m.poly(n[0] * out[0] + n[1] * out[1] + n[2] * out[2] < 0 ? [...corners].reverse() : corners, colour);
}

/** A skin of `sides` facets through the rings, its ends closed. `lump` pushes every second point out and the others in, as wool. */
function skin(m: Model, rings: Ring[], sides: number, paint: Paint | number, lump = 0): Model {
    const colour: Paint = typeof paint === 'number' ? () => paint : paint, last = rings.length - 1;
    const bend = (v: number, e: number) => Math.sign(v) * Math.abs(v) ** (2 / e);
    const points = rings.map((r, i) => Array.from({ length: sides }, (_, k): Vec => {
        const t = (2 * Math.PI * (k + 0.5)) / sides, p = (r.pitch * Math.PI) / 180, swell = i > 0 && i < last ? 1 + lump * ((i + k) % 2 ? 1 : -1) : 1;
        const s = bend(Math.sin(t), r.e), v = s * (s > 0 ? r.up : r.down) * swell;
        return [r.at[0] + bend(Math.cos(t), r.e) * r.w * swell, r.at[1] - Math.sin(p) * v, r.at[2] + Math.cos(p) * v];
    }));
    for (let i = 0; i < last; i++) for (let k = 0; k < sides; k++) {
        const j = (k + 1) % sides;
        face(m, [points[i][k], points[i][j], points[i + 1][j], points[i + 1][k]], colour(i, Math.sin((2 * Math.PI * (k + 1)) / sides)), along(rings[i].at, sub(rings[i + 1].at, rings[i].at), 0.5));
    }
    face(m, points[0], colour(-1, 0), rings[1].at);
    return face(m, points[last], colour(last, 0), rings[last - 1].at);
}

/** A flat diamond from a to its tip b, `width` across and `thick` along n: an ear, a beard, a short tail. */
function leaf(m: Model, a: Vec, b: Vec, width: number, thick: number, n: Vec, colour: number): Model {
    const d = sub(b, a), s = unit(cross(d, n)), c = along(a, d, 0.4), t = unit(n);
    const q = [along(c, s, width / 2), along(c, t, thick / 2), along(c, s, -width / 2), along(c, t, -thick / 2)];
    for (let k = 0; k < 4; k++) for (const end of [a, b]) face(m, [end, q[k], q[(k + 1) % 4]], colour, c);
    return m;
}

/** Bars through the points, each `widths` across at its point: a horn, a tail. */
function chain(m: Model, points: Vec[], widths: number[], colour: number, sides = 4): Model {
    for (let i = 0; i + 1 < points.length; i++) m.beam(points[i], points[i + 1], widths[i], colour, { sides, w1: widths[i + 1], capped: true });
    return m;
}

/** The rings of a body from rows of [y, z, half-width, up, down, pitch]. */
const along_y = (rows: number[][], e = 2): Ring[] => rows.map(([y, z, w, up, down, pitch = 0]) => ({ at: [0, y, z], w, up, down, pitch, e }));

/** Both legs of a pair at ±x, from rows of [y, z, half-width, reach to the front, reach to the back], top to hoof. */
function legs(m: Model, x: number, rows: number[][], sides: number, paint: Paint, e = 2): void {
    for (const s of [-1, 1]) skin(m, rows.map(([y, z, w, up, down]) => ({ at: [s * x, y, z], w, up, down, pitch: -90, e })), sides, paint);
}

/** A head from the back of the skull `at`, its axis pitched: `p` is the point s along the axis, x to the side and v above it; `rings` takes rows of [s, half-width, up, down]. */
function skull(at: Vec, pitch: number) {
    const c = Math.cos((pitch * Math.PI) / 180), s = Math.sin((pitch * Math.PI) / 180);
    const p = (d: number, x = 0, v = 0): Vec => [at[0] + x, at[1] + d * c - v * s, at[2] + d * s + v * c];
    return { p, axis: [0, c, s] as Vec, rings: (rows: number[][], e = 2): Ring[] => rows.map(([d, w, up, down]) => ({ at: p(d), w, up, down, pitch, e })) };
}

/** A paint of one colour for each band; the caps take the first and the last. */
const bands = (...colours: number[]): Paint => band => colours[Math.max(0, Math.min(colours.length - 1, band + 1))];
/** A paint with a belly: the facets that look down take `under`. */
const bellied = (coat: number, under: number): Paint => (_, top) => (top < -0.6 ? under : coat);

/** The same part at both sides: `f` takes −1 and 1. */
const both = (f: (s: number) => void) => [-1, 1].forEach(f);

const EYE = rgb('#1c1614'), HOOF = rgb('#2c2420'), HORN = rgb('#e2d6b8'), PINK = rgb('#d8a79a');
const COW = rgb('#9a7a5c'), COW_LIGHT = rgb('#c0a482'), CREAM = rgb('#e4d8c2'), NOSE = rgb('#3a2e2a');
const WOOL = rgb('#efe7d6'), FACE = rgb('#2d2828'), SHEEP_HOOF = rgb('#161212');
const GOAT = rgb('#87482a'), GOAT_DARK = rgb('#2e241e'), GOAT_CREAM = rgb('#e6dccb'), GOAT_HORN = rgb('#8a7a64');
const X: Vec = [1, 0, 0];

// ---- Style a: soft low-poly. Round skins of 10 sides, a large friendly head, short sturdy legs. ----

function cowA(): Model {
    const m = new Model(), h = skull([0, 0.66, 1.0], -30);
    skin(m, along_y([[-0.66, 0.74, 0.12, 0.14, 0.14], [-0.56, 0.72, 0.25, 0.25, 0.24], [-0.3, 0.72, 0.31, 0.28, 0.31], [0.05, 0.71, 0.32, 0.28, 0.33], [0.35, 0.73, 0.29, 0.27, 0.3], [0.52, 0.75, 0.22, 0.22, 0.24], [0.6, 0.76, 0.12, 0.13, 0.14]]), 10, bellied(COW, shade(COW, 0.85)));
    skin(m, along_y([[0.38, 0.78, 0.2, 0.2, 0.22, 30], [0.74, 0.93, 0.135, 0.14, 0.15, 25]]), 8, COW);
    skin(m, h.rings([[0, 0.1, 0.1, 0.1], [0.07, 0.165, 0.15, 0.15], [0.2, 0.17, 0.15, 0.16], [0.34, 0.13, 0.11, 0.13], [0.44, 0.125, 0.1, 0.12], [0.5, 0.09, 0.07, 0.08]]), 8, bands(COW, COW, COW, COW, CREAM, CREAM, NOSE));
    const leg = (y: number, k: number) => [[y, 0.62, 0.1 * k, 0.12 * k, 0.12 * k], [y, 0.36, 0.075, 0.085, 0.085], [y, 0.1, 0.065, 0.07, 0.07], [y, 0.085, 0.08, 0.09, 0.085], [y, 0, 0.085, 0.1, 0.09]];
    legs(m, 0.17, leg(0.36, 1), 6, bands(COW, COW, COW, HOOF));
    legs(m, 0.18, leg(-0.42, 1.15), 6, bands(COW, COW, COW, HOOF));
    both(s => {
        leaf(m, h.p(0.1, s * 0.13, 0.05), h.p(0.08, s * 0.4, 0), 0.14, 0.05, h.axis, COW);
        chain(m, [h.p(0.06, s * 0.1, 0.12), h.p(0.05, s * 0.2, 0.2), h.p(0.07, s * 0.22, 0.3)], [0.06, 0.045, 0.015], HORN);
        m.box(h.p(0.21, s * 0.165, 0.05), [0.03, 0.05, 0.05], EYE);
    });
    chain(m, [[0, -0.66, 0.9], [0, -0.75, 0.6], [0, -0.75, 0.42]], [0.04, 0.03, 0.03], COW);
    chain(m, [[0, -0.75, 0.44], [0, -0.75, 0.28]], [0.08, 0.03], NOSE);
    skin(m, along_y([[-0.25, 0.46, 0.11, 0.13, 0.13, -90], [-0.25, 0.36, 0.09, 0.11, 0.11, -90], [-0.25, 0.33, 0.05, 0.06, 0.06, -90]]), 6, PINK);
    return m;
}

function sheepA(): Model {
    const m = new Model(), h = skull([0, 0.56, 1.02], -35);
    skin(m, along_y([[-0.56, 0.72, 0.14, 0.13, 0.13], [-0.48, 0.71, 0.29, 0.25, 0.25], [-0.25, 0.7, 0.36, 0.29, 0.3], [0.05, 0.7, 0.37, 0.29, 0.31], [0.3, 0.71, 0.33, 0.27, 0.29], [0.45, 0.73, 0.22, 0.2, 0.22], [0.5, 0.74, 0.1, 0.1, 0.1]]), 12, bellied(WOOL, shade(WOOL, 0.88)), 0.07);
    skin(m, along_y([[0.36, 0.8, 0.2, 0.2, 0.2, 35], [0.6, 0.95, 0.15, 0.15, 0.15, 30]]), 8, WOOL);
    skin(m, h.rings([[0, 0.08, 0.08, 0.08], [0.06, 0.13, 0.12, 0.12], [0.18, 0.135, 0.12, 0.12], [0.3, 0.1, 0.09, 0.1], [0.38, 0.09, 0.075, 0.085], [0.42, 0.06, 0.05, 0.055]]), 8, FACE);
    const [, cy, cz] = h.p(0.09, 0, 0.11);
    skin(m, along_y([[cy - 0.09, cz, 0.06, 0.04, 0.04], [cy, cz, 0.13, 0.08, 0.05], [cy + 0.08, cz, 0.07, 0.04, 0.04]]), 8, WOOL);
    const leg = (y: number) => [[y, 0.55, 0.07, 0.08, 0.08], [y, 0.3, 0.05, 0.055, 0.055], [y, 0.09, 0.045, 0.05, 0.05], [y, 0.075, 0.055, 0.06, 0.06], [y, 0, 0.06, 0.07, 0.06]];
    legs(m, 0.17, leg(0.3), 6, bands(FACE, FACE, FACE, SHEEP_HOOF));
    legs(m, 0.17, leg(-0.36), 6, bands(FACE, FACE, FACE, SHEEP_HOOF));
    both(s => leaf(m, h.p(0.1, s * 0.1, 0.06), h.p(0.1, s * 0.31, -0.01), 0.11, 0.04, h.axis, FACE));
    chain(m, [[0, -0.54, 0.82], [0, -0.64, 0.7], [0, -0.66, 0.6]], [0.1, 0.1, 0.04], WOOL, 5);
    return m;
}

function goatA(): Model {
    const m = new Model(), h = skull([0, 0.5, 1.2], -30);
    skin(m, along_y([[-0.5, 0.78, 0.1, 0.1, 0.1], [-0.43, 0.77, 0.2, 0.2, 0.2], [-0.2, 0.76, 0.25, 0.23, 0.25], [0.1, 0.76, 0.25, 0.23, 0.26], [0.32, 0.77, 0.22, 0.23, 0.24], [0.44, 0.78, 0.15, 0.17, 0.18], [0.5, 0.79, 0.07, 0.08, 0.09]]), 10, bellied(GOAT, GOAT_CREAM));
    skin(m, along_y([[0.34, 0.84, 0.15, 0.17, 0.18, 50], [0.52, 1.1, 0.11, 0.12, 0.12, 40]]), 8, GOAT);
    skin(m, h.rings([[0, 0.08, 0.08, 0.08], [0.07, 0.135, 0.125, 0.125], [0.19, 0.14, 0.125, 0.13], [0.33, 0.1, 0.09, 0.1], [0.42, 0.09, 0.08, 0.09], [0.46, 0.06, 0.05, 0.055]]), 8, bands(GOAT, GOAT, GOAT, GOAT, GOAT_CREAM, GOAT_CREAM, NOSE));
    const leg = (y: number, k: number) => [[y, 0.62, 0.075 * k, 0.09 * k, 0.09 * k], [y, 0.36, 0.05, 0.055, 0.055], [y, 0.09, 0.04, 0.045, 0.045], [y, 0.075, 0.05, 0.055, 0.055], [y, 0, 0.055, 0.065, 0.055]];
    legs(m, 0.13, leg(0.3, 1), 6, bands(GOAT, GOAT, GOAT_DARK, HOOF));
    legs(m, 0.13, leg(-0.36, 1.15), 6, bands(GOAT, GOAT, GOAT_DARK, HOOF));
    both(s => {
        leaf(m, h.p(0.11, s * 0.1, 0.06), h.p(0.09, s * 0.34, 0.02), 0.11, 0.04, h.axis, GOAT);
        chain(m, [h.p(0.05, s * 0.07, 0.1), h.p(-0.03, s * 0.09, 0.22), h.p(-0.14, s * 0.11, 0.28), h.p(-0.24, s * 0.13, 0.27)], [0.06, 0.05, 0.035, 0.012], GOAT_HORN);
        m.box(h.p(0.2, s * 0.135, 0.05), [0.03, 0.05, 0.05], EYE);
    });
    leaf(m, h.p(0.34, 0, -0.07), h.p(0.31, 0, -0.29), 0.08, 0.05, X, GOAT_CREAM);
    leaf(m, [0, -0.44, 0.9], [0, -0.6, 1.08], 0.09, 0.05, X, GOAT);
    return m;
}

// ---- Style b: chunky toy. Bevelled blocks (square skins of 8 sides), an oversized head, stub legs, plain patches. ----

const BLOCK = 5;
/** A bevelled block along y from rows of [y, z, half-width, up, down]. */
const block = (m: Model, rows: number[][], paint: Paint | number) => skin(m, along_y(rows, BLOCK), 8, paint);

function cowB(): Model {
    const m = new Model(), h = skull([0, 0.3, 1.02], -10), nose = skull(h.p(0.36, 0, -0.08), -10);
    block(m, [[-0.5, 0.65, 0.26, 0.27, 0.27], [-0.43, 0.65, 0.33, 0.35, 0.35], [0.3, 0.65, 0.33, 0.35, 0.35], [0.38, 0.65, 0.26, 0.27, 0.27]], bellied(COW, COW_LIGHT));
    skin(m, h.rings([[0, 0.2, 0.19, 0.19], [0.07, 0.26, 0.25, 0.25], [0.36, 0.26, 0.25, 0.25], [0.42, 0.2, 0.19, 0.19]], BLOCK), 8, (band, top) => (band === 1 && top > 0.6 ? COW_LIGHT : COW));
    skin(m, nose.rings([[0, 0.22, 0.15, 0.15], [0.1, 0.22, 0.15, 0.15], [0.15, 0.17, 0.11, 0.11]], BLOCK), 8, CREAM);
    const leg = (y: number) => [[y, 0.4, 0.11, 0.11, 0.11], [y, 0.12, 0.1, 0.1, 0.1], [y, 0.105, 0.115, 0.115, 0.115], [y, 0, 0.115, 0.115, 0.115]];
    legs(m, 0.19, leg(0.22), 8, bands(COW, COW, HOOF), BLOCK);
    legs(m, 0.19, leg(-0.3), 8, bands(COW, COW, HOOF), BLOCK);
    both(s => {
        leaf(m, h.p(0.1, s * 0.2, 0.14), h.p(0.1, s * 0.45, 0.14), 0.15, 0.07, h.axis, COW);
        chain(m, [h.p(0.14, s * 0.14, 0.23), h.p(0.14, s * 0.21, 0.39)], [0.09, 0.04], HORN);
        m.box(h.p(0.26, s * 0.26, 0.07), [0.04, 0.07, 0.08], EYE);
        m.box(nose.p(0.15, s * 0.08, 0.02), [0.04, 0.03, 0.04], NOSE);
    });
    chain(m, [[0, -0.5, 0.9], [0, -0.59, 0.55]], [0.05, 0.05], COW);
    m.box([0, -0.59, 0.5], [0.09, 0.09, 0.14], NOSE).box([0, -0.18, 0.3], [0.22, 0.2, 0.1], PINK);
    return m;
}

function sheepB(): Model {
    const m = new Model(), h = skull([0, 0.3, 0.98], -15);
    block(m, [[-0.48, 0.66, 0.28, 0.27, 0.27], [-0.4, 0.66, 0.37, 0.35, 0.35], [0.3, 0.66, 0.37, 0.35, 0.35], [0.38, 0.66, 0.28, 0.27, 0.27]], bellied(WOOL, shade(WOOL, 0.88)));
    skin(m, h.rings([[0, 0.15, 0.14, 0.14], [0.06, 0.2, 0.19, 0.19], [0.3, 0.2, 0.19, 0.2], [0.4, 0.15, 0.13, 0.14], [0.44, 0.1, 0.08, 0.09]], BLOCK), 8, FACE);
    const [, cy, cz] = h.p(0.12, 0, 0.2);
    block(m, [[cy - 0.12, cz, 0.12, 0.07, 0.05], [cy - 0.07, cz, 0.17, 0.1, 0.07], [cy + 0.08, cz, 0.17, 0.1, 0.07], [cy + 0.12, cz, 0.12, 0.07, 0.05]], WOOL);
    const leg = (y: number) => [[y, 0.4, 0.085, 0.085, 0.085], [y, 0.1, 0.075, 0.075, 0.075], [y, 0.085, 0.09, 0.09, 0.09], [y, 0, 0.09, 0.09, 0.09]];
    legs(m, 0.2, leg(0.2), 8, bands(FACE, FACE, SHEEP_HOOF), BLOCK);
    legs(m, 0.2, leg(-0.28), 8, bands(FACE, FACE, SHEEP_HOOF), BLOCK);
    both(s => leaf(m, h.p(0.1, s * 0.16, 0.08), h.p(0.1, s * 0.4, 0), 0.13, 0.06, h.axis, FACE));
    block(m, [[-0.58, 0.76, 0.05, 0.06, 0.06], [-0.55, 0.76, 0.08, 0.09, 0.09], [-0.44, 0.76, 0.08, 0.09, 0.09]], WOOL);
    return m;
}

function goatB(): Model {
    const m = new Model(), h = skull([0, 0.28, 1.22], -15);
    block(m, [[-0.46, 0.7, 0.2, 0.22, 0.22], [-0.39, 0.7, 0.27, 0.3, 0.3], [0.28, 0.7, 0.27, 0.3, 0.3], [0.35, 0.7, 0.2, 0.22, 0.22]], bellied(GOAT, GOAT_CREAM));
    skin(m, along_y([[0.2, 0.82, 0.17, 0.17, 0.17, 60], [0.38, 1.14, 0.15, 0.15, 0.15, 60]], BLOCK), 8, GOAT);
    skin(m, h.rings([[0, 0.15, 0.14, 0.14], [0.06, 0.2, 0.18, 0.18], [0.28, 0.2, 0.18, 0.19], [0.4, 0.14, 0.11, 0.13], [0.45, 0.1, 0.08, 0.09]], BLOCK), 8, bands(GOAT, GOAT, GOAT, GOAT_CREAM, GOAT_CREAM, NOSE));
    const leg = (y: number) => [[y, 0.5, 0.085, 0.085, 0.085], [y, 0.28, 0.075, 0.075, 0.075], [y, 0.11, 0.07, 0.07, 0.07], [y, 0.095, 0.085, 0.085, 0.085], [y, 0, 0.085, 0.085, 0.085]];
    legs(m, 0.15, leg(0.2), 8, bands(GOAT, GOAT, GOAT_DARK, HOOF), BLOCK);
    legs(m, 0.15, leg(-0.28), 8, bands(GOAT, GOAT, GOAT_DARK, HOOF), BLOCK);
    both(s => {
        leaf(m, h.p(0.1, s * 0.16, 0.06), h.p(0.08, s * 0.42, 0.02), 0.13, 0.06, h.axis, GOAT);
        chain(m, [h.p(0.08, s * 0.1, 0.16), h.p(-0.04, s * 0.13, 0.36), h.p(-0.2, s * 0.15, 0.42)], [0.09, 0.07, 0.03], GOAT_HORN);
        m.box(h.p(0.21, s * 0.2, 0.05), [0.03, 0.06, 0.07], EYE);
    });
    leaf(m, h.p(0.33, 0, -0.12), h.p(0.3, 0, -0.4), 0.11, 0.08, X, GOAT_CREAM);
    leaf(m, [0, -0.4, 0.9], [0, -0.56, 1.14], 0.11, 0.08, X, GOAT);
    return m;
}

// ---- Style c: natural faceted. Real proportions: a backline and a belly, hips and shoulders, legs with a knee and a hock. ----

/** A front and a hind pair of legs with a knee and a hock: `x` and `y` of each pair, the top of the leg at `top`, the bone `w` thick. */
function jointed(m: Model, front: [number, number], hind: [number, number], top: number, w: number, paint: Paint): void {
    const [fx, fy] = front, [hx, hy] = hind, knee = 0.44 * top, k = w / 0.04;
    legs(m, fx, [[fy, top, 0.09 * k, 0.11 * k, 0.11 * k], [fy, 0.72 * top, 0.065 * k, 0.07 * k, 0.07 * k], [fy + 0.01, knee, 0.055 * k, 0.058 * k, 0.055 * k], [fy, 0.09, w, w, w], [fy, 0.07, 0.052 * k, 0.058 * k, 0.05 * k], [fy + 0.01, 0, 0.058 * k, 0.075 * k, 0.055 * k]], 6, paint);
    legs(m, hx, [[hy, top + 0.06, 0.1 * k, 0.16 * k, 0.14 * k], [hy + 0.02, 0.72 * top, 0.07 * k, 0.1 * k, 0.09 * k], [hy - 0.07, knee + 0.03, 0.045 * k, 0.05 * k, 0.05 * k], [hy - 0.04, 0.09, w, w, w], [hy - 0.04, 0.07, 0.052 * k, 0.058 * k, 0.05 * k], [hy - 0.03, 0, 0.058 * k, 0.075 * k, 0.055 * k]], 6, paint);
}

function cowC(): Model {
    const m = new Model(), h = skull([0, 0.73, 1.02], -32);
    skin(m, along_y([[-0.62, 0.78, 0.08, 0.1, 0.12], [-0.56, 0.73, 0.22, 0.24, 0.26], [-0.38, 0.71, 0.29, 0.28, 0.31], [-0.05, 0.69, 0.33, 0.27, 0.33], [0.25, 0.7, 0.3, 0.28, 0.3], [0.42, 0.72, 0.24, 0.28, 0.3], [0.55, 0.74, 0.16, 0.2, 0.26], [0.6, 0.74, 0.07, 0.1, 0.12]], 2.6), 10, (_, top) => (top < -0.6 ? shade(COW, 0.85) : top > 0.9 ? COW_LIGHT : COW));
    skin(m, along_y([[0.45, 0.78, 0.18, 0.22, 0.27, 25], [0.62, 0.86, 0.14, 0.16, 0.21, 15], [0.77, 0.92, 0.11, 0.11, 0.14, 5]]), 8, COW);
    skin(m, h.rings([[0, 0.08, 0.08, 0.08], [0.06, 0.14, 0.13, 0.12], [0.18, 0.145, 0.13, 0.14], [0.35, 0.11, 0.095, 0.115], [0.44, 0.105, 0.085, 0.105], [0.49, 0.07, 0.06, 0.07]]), 8, bands(COW, COW, COW, COW, CREAM, CREAM, NOSE));
    jointed(m, [0.16, 0.4], [0.17, -0.4], 0.62, 0.048, bands(COW, COW, COW, COW, HOOF));
    both(s => {
        leaf(m, h.p(0.11, s * 0.11, 0.05), h.p(0.09, s * 0.34, 0.03), 0.12, 0.04, h.axis, COW);
        chain(m, [h.p(0.06, s * 0.09, 0.11), h.p(0.08, s * 0.2, 0.17), h.p(0.13, s * 0.22, 0.27)], [0.055, 0.04, 0.012], HORN);
        m.box(h.p(0.2, s * 0.14, 0.05), [0.025, 0.045, 0.045], EYE);
    });
    chain(m, [[0, -0.61, 0.92], [0, -0.69, 0.7], [0, -0.68, 0.42]], [0.045, 0.035, 0.03], COW);
    chain(m, [[0, -0.68, 0.44], [0, -0.68, 0.27]], [0.08, 0.025], NOSE);
    skin(m, along_y([[-0.3, 0.42, 0.09, 0.11, 0.11, -90], [-0.3, 0.33, 0.07, 0.09, 0.09, -90], [-0.3, 0.31, 0.04, 0.05, 0.05, -90]]), 6, PINK);
    // A shorter barrel behind the shoulder: the full length reads as a horse.
    return m.map(([x, y, z]) => [x, y < 0.42 ? 0.42 + (y - 0.42) * 0.85 : y, z]);
}

function sheepC(): Model {
    const m = new Model(), h = skull([0, 0.58, 1.05], -32);
    skin(m, along_y([[-0.58, 0.76, 0.1, 0.1, 0.1], [-0.5, 0.73, 0.25, 0.24, 0.25], [-0.3, 0.72, 0.33, 0.27, 0.3], [0, 0.71, 0.35, 0.27, 0.32], [0.28, 0.72, 0.32, 0.27, 0.3], [0.44, 0.75, 0.22, 0.23, 0.24], [0.5, 0.76, 0.09, 0.1, 0.1]]), 10, bellied(WOOL, shade(WOOL, 0.88)), 0.05);
    skin(m, along_y([[0.38, 0.82, 0.16, 0.18, 0.2, 35], [0.6, 0.97, 0.11, 0.12, 0.12, 25]]), 8, WOOL);
    skin(m, h.rings([[0, 0.07, 0.07, 0.07], [0.06, 0.12, 0.11, 0.11], [0.17, 0.125, 0.115, 0.115], [0.31, 0.09, 0.09, 0.095], [0.39, 0.08, 0.07, 0.08], [0.43, 0.055, 0.05, 0.05]]), 8, FACE);
    jointed(m, [0.15, 0.32], [0.16, -0.36], 0.58, 0.038, bands(FACE, FACE, FACE, FACE, SHEEP_HOOF));
    both(s => leaf(m, h.p(0.1, s * 0.09, 0.06), h.p(0.1, s * 0.3, 0), 0.1, 0.04, h.axis, FACE));
    chain(m, [[0, -0.55, 0.85], [0, -0.63, 0.62]], [0.09, 0.05], WOOL, 5);
    return m;
}

function goatC(): Model {
    const m = new Model(), h = skull([0, 0.55, 1.17], -30);
    skin(m, along_y([[-0.52, 0.82, 0.07, 0.08, 0.09], [-0.46, 0.78, 0.18, 0.19, 0.21], [-0.3, 0.76, 0.23, 0.22, 0.25], [0, 0.74, 0.26, 0.22, 0.28], [0.26, 0.75, 0.22, 0.24, 0.26], [0.4, 0.78, 0.17, 0.22, 0.24], [0.48, 0.8, 0.07, 0.1, 0.12]], 2.4), 10, (_, top) => (top < -0.6 ? GOAT_CREAM : top > 0.9 ? GOAT_DARK : GOAT));
    skin(m, along_y([[0.36, 0.84, 0.14, 0.16, 0.19, 45], [0.5, 1.0, 0.11, 0.12, 0.14, 45], [0.58, 1.1, 0.09, 0.1, 0.1, 30]]), 8, GOAT);
    skin(m, h.rings([[0, 0.065, 0.065, 0.065], [0.06, 0.115, 0.1, 0.1], [0.17, 0.12, 0.105, 0.11], [0.31, 0.085, 0.08, 0.09], [0.4, 0.07, 0.065, 0.075], [0.44, 0.045, 0.04, 0.045]]), 8, bands(GOAT, GOAT, GOAT, GOAT, GOAT_CREAM, GOAT_CREAM, NOSE));
    jointed(m, [0.12, 0.3], [0.13, -0.36], 0.64, 0.034, bands(GOAT, GOAT, GOAT, GOAT_DARK, HOOF));
    both(s => {
        leaf(m, h.p(0.1, s * 0.09, 0.05), h.p(0.08, s * 0.3, 0.03), 0.09, 0.035, h.axis, GOAT);
        chain(m, [h.p(0.05, s * 0.06, 0.09), h.p(-0.02, s * 0.08, 0.2), h.p(-0.12, s * 0.1, 0.27), h.p(-0.22, s * 0.12, 0.27)], [0.055, 0.045, 0.03, 0.01], GOAT_HORN);
        m.box(h.p(0.17, s * 0.115, 0.04), [0.025, 0.04, 0.04], EYE);
    });
    leaf(m, h.p(0.32, 0, -0.07), h.p(0.29, 0, -0.27), 0.07, 0.04, X, GOAT_CREAM);
    leaf(m, [0, -0.48, 0.92], [0, -0.6, 1.07], 0.07, 0.035, X, GOAT);
    return m;
}

/** The candidates by `<animal>-<style>`: the showroom stands them beside today's breeds. */
export const CANDIDATES: Record<string, Candidate> = {
    'cow-a': { height: 1.4, build: cowA }, 'cow-b': { height: 1.4, build: cowB }, 'cow-c': { height: 1.4, build: cowC },
    'sheep-a': { height: 0.95, build: sheepA }, 'sheep-b': { height: 0.95, build: sheepB }, 'sheep-c': { height: 0.95, build: sheepC },
    'goat-a': { height: 1.05, build: goatA }, 'goat-b': { height: 1.05, build: goatB }, 'goat-c': { height: 1.05, build: goatC },
};
