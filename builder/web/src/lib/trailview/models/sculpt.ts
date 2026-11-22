// The modelling kit of the soft low-poly style (docs/content: the model style guide): round skins lofted through rings,
// flat leaves and chains of bars, in flat faces of one colour each. Every face turns away from a point inside its part,
// so no part needs a hand-made winding. A sculpt has a tier: tier 1 builds the same parts with fewer sides and leaves
// out the fine details, for the far tier of a model.
import { geometry, Model, type Vec } from './flat';
import type { FloatMesh } from './model-file';

/** A ring of a skin: its middle, its half-width, its reach above and below the middle, the pitch of its axis above +y in degrees, and how square it is (2 is an ellipse). */
export interface Ring { at: Vec; w: number; up: number; down: number; pitch: number; e: number }
/**
 * The colour of a face from its band (−1 and the band count are the end caps), from how far up (−1 to 1) its facet looks,
 * and from its place around the ring: 0 at +x, 0.25 at the top, 0.5 at −x.
 */
export type Paint = (band: number, top: number, around: number) => number;

export const sub = (a: Vec, b: Vec): Vec => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
export const along = (a: Vec, d: Vec, k: number): Vec => [a[0] + d[0] * k, a[1] + d[1] * k, a[2] + d[2] * k];
const cross = (a: Vec, b: Vec): Vec => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const unit = (a: Vec): Vec => along([0, 0, 0], a, 1 / (Math.hypot(...a) || 1));
const bend = (v: number, e: number) => Math.sign(v) * Math.abs(v) ** (2 / e);

/** A paint of one colour for each band; the caps take the first and the last. */
export const bands = (...colours: number[]): Paint => band => colours[Math.max(0, Math.min(colours.length - 1, band + 1))];
/** A paint with a belly: the facets that look down take `under`. */
export const bellied = (coat: number, under: number): Paint => (_, top) => (top < -0.6 ? under : coat);
/** The same part at both sides: `f` takes −1 and 1. */
export const both = (f: (s: number) => void) => [-1, 1].forEach(f);

/** The rings of a part along y from rows of [y, z, half-width, up, down, pitch]. */
export const alongY = (rows: number[][], e = 2): Ring[] => rows.map(([y, z, w, up, down, pitch = 0]) => ({ at: [0, y, z], w, up, down, pitch, e }));

/**
 * A frame along an axis from `at`, pitched `pitch` degrees above +y: `p(d, x, v)` is the point d along the axis, x to the
 * side and v above it; `rings` takes rows of [d, half-width, up, down]. A head, a neck or a tail hangs from one.
 */
export function axis(at: Vec, pitch: number) {
    const c = Math.cos((pitch * Math.PI) / 180), s = Math.sin((pitch * Math.PI) / 180);
    const p = (d: number, x = 0, v = 0): Vec => [at[0] + x, at[1] + d * c - v * s, at[2] + d * s + v * c];
    return { p, dir: [0, c, s] as Vec, rings: (rows: number[][], e = 2): Ring[] => rows.map(([d, w, up, down]) => ({ at: p(d), w, up, down, pitch, e })) };
}

/**
 * The rings of a part through points of one bend in the y-z plane, each ring square to the path, from rows of
 * [half-width, up, down]: a bent neck, a folded leg.
 */
export function through(points: Vec[], rows: number[][], e = 2): Ring[] {
    return points.map((at, i) => {
        const a = points[Math.max(0, i - 1)], b = points[Math.min(points.length - 1, i + 1)], [w, up, down] = rows[i];
        return { at, w, up, down, pitch: (Math.atan2(b[2] - a[2], b[1] - a[1]) * 180) / Math.PI, e };
    });
}

/** A model in the making, at a tier: 0 near, 1 far. */
export class Sculpt {
    readonly model = new Model();
    constructor(readonly tier = 0) {}

    /** Sides of a part that has n at tier 0: 4 keep a round part the same at both sides, 3 are enough for a thin one. */
    sides(n: number): number {
        return this.tier ? (n < 8 ? 3 : 4) : n;
    }

    /** Fine details, which only tier 0 builds. */
    fine(f: () => void): this {
        if (!this.tier) f();
        return this;
    }

    /** A convex face whose normal points away from `inside`. */
    face(corners: Vec[], colour: number, inside: Vec): this {
        const n = cross(sub(corners[1], corners[0]), sub(corners[2], corners[0]));
        const mid = corners.reduce((sum, p) => along(sum, p, 1 / corners.length), [0, 0, 0] as Vec), out = sub(mid, inside);
        this.model.poly(n[0] * out[0] + n[1] * out[1] + n[2] * out[2] < 0 ? [...corners].reverse() : corners, colour);
        return this;
    }

    /**
     * A skin of `sides` facets (fewer at tier 1) through the rings, its ends closed; a ring of few sides grows to keep the
     * area of its ellipse. `lump` pushes every second point out and the others in, as wool.
     */
    skin(rings: Ring[], sides: number, paint: Paint | number, lump = 0): this {
        const colour: Paint = typeof paint === 'number' ? () => paint : paint, last = rings.length - 1, n = this.sides(sides), full = Math.sqrt(Math.PI / ((n / 2) * Math.sin((2 * Math.PI) / n)));
        const points = rings.map((r, i) => Array.from({ length: n }, (_, k): Vec => {
            const t = (2 * Math.PI * (k + 0.5)) / n, p = (r.pitch * Math.PI) / 180, swell = full * (i > 0 && i < last ? 1 + lump * ((i + k) % 2 ? 1 : -1) : 1);
            const s = bend(Math.sin(t), r.e), v = s * (s > 0 ? r.up : r.down) * swell;
            return [r.at[0] + bend(Math.cos(t), r.e) * r.w * swell, r.at[1] - Math.sin(p) * v, r.at[2] + Math.cos(p) * v];
        }));
        for (let i = 0; i < last; i++) for (let k = 0; k < n; k++) {
            const j = (k + 1) % n;
            this.face([points[i][k], points[i][j], points[i + 1][j], points[i + 1][k]], colour(i, Math.sin((2 * Math.PI * (k + 1)) / n), ((k + 1) / n) % 1), along(rings[i].at, sub(rings[i + 1].at, rings[i].at), 0.5));
        }
        this.face(points[0], colour(-1, 0, 0), rings[1].at);
        return this.face(points[last], colour(last, 0, 0), rings[last - 1].at);
    }

    /** A flat diamond from a to its tip b, `width` across and `thick` along n: an ear, a beard, a short tail. */
    leaf(a: Vec, b: Vec, width: number, thick: number, n: Vec, colour: number): this {
        const d = sub(b, a), s = unit(cross(d, n)), c = along(a, d, 0.4), t = unit(n);
        const q = [along(c, s, width / 2), along(c, t, thick / 2), along(c, s, -width / 2), along(c, t, -thick / 2)];
        for (let k = 0; k < 4; k++) for (const end of [a, b]) this.face([end, q[k], q[(k + 1) % 4]], colour, c);
        return this;
    }

    /** Bars through the points, each `widths` across at its point, of `sides` sides: a horn, a tail. Tier 1 has fewer sides and goes through every second point and the last. */
    chain(points: Vec[], widths: number[], colour: number, sides = 4): this {
        const keep = points.map((_, i) => i).filter(i => !this.tier || i % 2 === 0 || i === points.length - 1);
        for (let k = 0; k + 1 < keep.length; k++) {
            const a = points[keep[k]], b = points[keep[k + 1]], first = this.model.triangles;
            this.model.beam(a, b, widths[keep[k]], colour, { sides: this.sides(sides), w1: widths[keep[k + 1]], capped: true });
            this.outward(first, along(a, sub(b, a), 0.5));
        }
        return this;
    }

    /** Turns each triangle from `first` on away from `inside`: Model.beam winds its start cap inward. */
    private outward(first: number, inside: Vec): void {
        const p = this.model.points;
        for (let i = 9 * first; i < p.length; i += 9) {
            const a: Vec = [p[i], p[i + 1], p[i + 2]], b: Vec = [p[i + 3], p[i + 4], p[i + 5]], c: Vec = [p[i + 6], p[i + 7], p[i + 8]];
            const n = cross(sub(b, a), sub(c, a)), out = sub(along(along(a, b, 1), c, 1).map(v => v / 3) as Vec, inside);
            if (n[0] * out[0] + n[1] * out[1] + n[2] * out[2] < 0) p.splice(i + 3, 6, ...c, ...b);
        }
    }

    /** A box about its middle: an eye, a nostril. */
    box(at: Vec, size: Vec, colour: number): this {
        this.model.box(at, size, colour);
        return this;
    }
}

/**
 * The mesh of a model as the instances read it: each polygon lit flat by one normal (Newell's, so a lofted quad that is
 * not quite planar draws as one facet), its corners kept once.
 */
export function mesh(model: Model): FloatMesh {
    const flat = geometry(model), p = model.points, normals = new Map<number, Vec>();
    model.faces.forEach((face, t) => {
        const n = normals.get(face) ?? [0, 0, 0], i = 9 * t;
        const c = cross([p[i + 3] - p[i], p[i + 4] - p[i + 1], p[i + 5] - p[i + 2]], [p[i + 6] - p[i], p[i + 7] - p[i + 1], p[i + 8] - p[i + 2]]);
        normals.set(face, along(n, c, 1));
    });
    const at = new Map<string, number>(), out = { position: [] as number[], normal: [] as number[], color: [] as number[], index: [] as number[] };
    for (let v = 0; v < flat.position.length / 3; v++) {
        const position = [0, 1, 2].map(d => flat.position[3 * v + d]), normal = unit(normals.get(model.faces[Math.floor(v / 3)])!), color = [0, 1, 2].map(d => flat.color[3 * v + d]);
        const key = [...position, ...normal, ...color].join();
        let k = at.get(key);
        if (k === undefined) {
            at.set(key, (k = out.position.length / 3));
            out.position.push(...position);
            out.normal.push(...normal);
            out.color.push(...color);
        }
        out.index.push(k);
    }
    return out;
}
