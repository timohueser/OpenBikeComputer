// The parts of the procedural models: flat triangles in one colour each, in metres, z up. A model has no
// normals: the shaders light each face flat, from its own slope.

export type Vec = [number, number, number];

/** sRGB bytes of a `#rrggbb` colour, red in the low byte. */
export const hex = (h: string) => parseInt(h.slice(1, 3), 16) | (parseInt(h.slice(3, 5), 16) << 8) | (parseInt(h.slice(5, 7), 16) << 16);
/** A colour k times as bright. */
export const shade = (c: number, k: number) => [0, 8, 16].reduce((out, bit) => out | (Math.min(255, Math.round(((c >> bit) & 255) * k)) << bit), 0);
/** The item of a list that a seed picks: consecutive seeds pick different items. */
export const pick = <T>(list: T[], seed: number, salt = 0) => list[Math.abs(Math.floor(seed) + salt) % list.length];

/** Seeded random numbers in [0, 1), the same on every load. */
export function random(seed = 1): () => number {
    let s = (seed * 7919 + 1) % 2147483647 || 1;
    return () => (s = (s * 16807) % 2147483647) / 2147483647;
}

/** Triangles: nine coordinates and one colour each. */
export class Model {
    readonly points: number[] = [];
    readonly colours: number[] = [];

    get triangles(): number {
        return this.colours.length;
    }

    tri(a: Vec, b: Vec, c: Vec, colour: number): this {
        this.points.push(...a, ...b, ...c);
        this.colours.push(colour);
        return this;
    }

    /** A convex polygon, as a fan. */
    poly(corners: Vec[], colour: number): this {
        for (let i = 1; i + 1 < corners.length; i++) this.tri(corners[0], corners[i], corners[i + 1], colour);
        return this;
    }

    add(other: Model): this {
        this.points.push(...other.points);
        this.colours.push(...other.colours);
        return this;
    }

    /** Each point moved by `f`. */
    map(f: (p: Vec) => Vec): this {
        const p = this.points;
        for (let i = 0; i < p.length; i += 3) [p[i], p[i + 1], p[i + 2]] = f([p[i], p[i + 1], p[i + 2]]);
        return this;
    }

    /** Turned by `angle` radians about z, counter-clockwise from above, then moved by `at`. */
    place(angle: number, at: Vec = [0, 0, 0], scale = 1): this {
        const c = Math.cos(angle), s = Math.sin(angle);
        return this.map(([x, y, z]) => [at[0] + scale * (c * x - s * y), at[1] + scale * (s * x + c * y), at[2] + scale * z]);
    }

    /** A box about its middle `at`, in one colour or one per face: +x, −x, +y, −y, +z, −z. */
    box(at: Vec, [sx, sy, sz]: Vec, colour: number | number[]): this {
        const c = (k: number) => (Array.isArray(colour) ? colour[k] : colour);
        const p = (i: number, j: number, k: number): Vec => [at[0] + (i - 0.5) * sx, at[1] + (j - 0.5) * sy, at[2] + (k - 0.5) * sz];
        this.poly([p(1, 0, 0), p(1, 1, 0), p(1, 1, 1), p(1, 0, 1)], c(0)).poly([p(0, 0, 0), p(0, 0, 1), p(0, 1, 1), p(0, 1, 0)], c(1));
        this.poly([p(0, 1, 0), p(0, 1, 1), p(1, 1, 1), p(1, 1, 0)], c(2)).poly([p(0, 0, 0), p(1, 0, 0), p(1, 0, 1), p(0, 0, 1)], c(3));
        return this.poly([p(0, 0, 1), p(1, 0, 1), p(1, 1, 1), p(0, 1, 1)], c(4)).poly([p(0, 0, 0), p(0, 1, 0), p(1, 1, 0), p(1, 0, 0)], c(5));
    }

    /** A bar from a to b with `sides` sides, `w` across, open at its ends unless capped; `w1` across at b. */
    beam(a: Vec, b: Vec, w: number, colour: number, { sides = 4, w1 = w, capped = false } = {}): this {
        const d = norm(sub(b, a)), u = norm(cross(d, Math.abs(d[2]) < 0.95 ? [0, 0, 1] : [1, 0, 0])), v = cross(d, u);
        const ring = (at: Vec, r: number) => Array.from({ length: sides }, (_, k): Vec => {
            const t = (2 * Math.PI * (k + 0.5)) / sides, x = (r / 2) * Math.cos(t) * Math.SQRT2, y = (r / 2) * Math.sin(t) * Math.SQRT2;
            return [at[0] + u[0] * x + v[0] * y, at[1] + u[1] * x + v[1] * y, at[2] + u[2] * x + v[2] * y];
        });
        const p = ring(a, w), q = ring(b, w1);
        for (let k = 0; k < sides; k++) this.poly([p[k], p[(k + 1) % sides], q[(k + 1) % sides], q[k]], colour);
        if (capped) this.poly(p, colour).poly(q, colour);
        return this;
    }

    /** A convex profile in the x–z plane drawn along y from y0 to y1, its ends closed. */
    prism(profile: [number, number][], y0: number, y1: number, colour: number): this {
        const at = (y: number) => profile.map(([x, z]): Vec => [x, y, z]);
        const a = at(y0), b = at(y1), n = profile.length;
        for (let k = 0; k < n; k++) this.poly([a[k], a[(k + 1) % n], b[(k + 1) % n], b[k]], colour);
        return this.poly(a, colour).poly(b, colour);
    }

    /**
     * An outline in the x–y plane swept up through rings of [z, scale], closed at both ends. `side` paints
     * the face of each band from the band, the face's outward normal and its middle in the outline.
     */
    loft(foot: [number, number][], rings: [number, number][], side: (band: number, n: [number, number], m: [number, number]) => number, bottom: number, top: number): this {
        const at = (i: number, [z, s]: [number, number]): Vec => [foot[i % foot.length][0] * s, foot[i % foot.length][1] * s, z];
        rings.slice(1).forEach((ring, band) => {
            const low = rings[band];
            foot.forEach(([ax, ay], i) => {
                const [bx, by] = foot[(i + 1) % foot.length], l = Math.hypot(bx - ax, by - ay) || 1;
                this.poly([at(i, low), at(i + 1, low), at(i + 1, ring), at(i, ring)], side(band, [(by - ay) / l, (ax - bx) / l], [(ax + bx) / 2, (ay + by) / 2]));
            });
        });
        this.poly(foot.map((_, i) => at(i, rings[0])), bottom);
        return this.poly(foot.map((_, i) => at(i, rings[rings.length - 1])), top);
    }
}

/** A rounded rectangle, counter-clockwise from above, its corners in `steps` facets of radius r. */
export function outline(hx: number, hy: number, r: number, steps: number): [number, number][] {
    const out: [number, number][] = [];
    [[1, 1], [-1, 1], [-1, -1], [1, -1]].forEach(([sx, sy], k) => {
        for (let i = 0; i <= steps; i++) {
            const a = ((k + i / steps) * Math.PI) / 2;
            out.push([sx * (hx - r) + r * Math.cos(a), sy * (hy - r) + r * Math.sin(a)]);
        }
    });
    return out;
}

/** A gable roof on walls `width` × `length` whose tops are at z, the ridge along y, with its gables in `wall`. */
export function roof(m: Model, [x, y, z]: Vec, width: number, length: number, pitch: number, eave: number, colour: number, wall: number): Model {
    const k = Math.tan((pitch * Math.PI) / 180), h = width / 2, ridge = h * k, t = 0.3;
    for (const s of [-1, 1]) {
        const foot = s * (h + eave), drop = -eave * k;
        m.prism([[x + foot, z + drop], [x, z + ridge], [x, z + ridge + t], [x + foot, z + drop + t]], y - length / 2 - eave, y + length / 2 + eave, colour);
    }
    return m.prism([[x - h, z], [x + h, z], [x, z + ridge]], y - length / 2, y + length / 2, wall);
}

const sub = (a: Vec, b: Vec): Vec => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const cross = (a: Vec, b: Vec): Vec => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const norm = (a: Vec): Vec => {
    const l = Math.hypot(...a) || 1;
    return [a[0] / l, a[1] / l, a[2] / l];
};
