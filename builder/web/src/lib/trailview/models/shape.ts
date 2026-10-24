// Soft primitives for the models, without three.js: the prototype's src/models/shape.ts on plain
// arrays. A builder paints each part with vertex colours and merges the parts into one geometry with
// positions, normals and colours. z is up. Colours are linear, as three.js holds a colour from hex.

export type Vec = [number, number, number];

/** A mutable 3-vector with the methods of three.js's Vector3 that the models use. */
export class V {
    constructor(public x = 0, public y = 0, public z = 0) {}
    set(x: number, y: number, z: number): this { this.x = x; this.y = y; this.z = z; return this; }
    setZ(z: number): this { this.z = z; return this; }
    clone(): V { return new V(this.x, this.y, this.z); }
    add(v: V): this { return this.set(this.x + v.x, this.y + v.y, this.z + v.z); }
    sub(v: V): this { return this.set(this.x - v.x, this.y - v.y, this.z - v.z); }
    subVectors(a: V, b: V): this { return this.set(a.x - b.x, a.y - b.y, a.z - b.z); }
    addScaledVector(v: V, s: number): this { return this.set(this.x + v.x * s, this.y + v.y * s, this.z + v.z * s); }
    multiplyScalar(s: number): this { return this.set(this.x * s, this.y * s, this.z * s); }
    divideScalar(s: number): this { return this.multiplyScalar(1 / s); }
    divide(v: V): this { return this.set(this.x / v.x, this.y / v.y, this.z / v.z); }
    lerp(v: V, t: number): this { return this.set(this.x + (v.x - this.x) * t, this.y + (v.y - this.y) * t, this.z + (v.z - this.z) * t); }
    dot(v: V): number { return this.x * v.x + this.y * v.y + this.z * v.z; }
    lengthSq(): number { return this.dot(this); }
    length(): number { return Math.sqrt(this.lengthSq()); }
    normalize(): this { return this.divideScalar(this.length() || 1); }
    setLength(l: number): this { return this.normalize().multiplyScalar(l); }
    negate(): this { return this.multiplyScalar(-1); }
    cross(v: V): this { return this.set(this.y * v.z - this.z * v.y, this.z * v.x - this.x * v.z, this.x * v.y - this.y * v.x); }
    toArray(): Vec { return [this.x, this.y, this.z]; }
    crossVectors(a: V, b: V): this { return this.copy(a).cross(b); }
    copy(v: V): this { return this.set(v.x, v.y, v.z); }
    multiply(v: V): this { return this.set(this.x * v.x, this.y * v.y, this.z * v.z); }
    distanceTo(v: V): number { return Math.hypot(this.x - v.x, this.y - v.y, this.z - v.z); }
    projectOnPlane(n: V): this { return this.addScaledVector(n, -this.dot(n) / n.lengthSq()); }
    /** Turns the vector by `angle` about the unit axis, by Rodrigues' formula. */
    applyAxisAngle(axis: V, angle: number): this {
        const c = Math.cos(angle), s = Math.sin(angle), k = axis, d = k.dot(this), x = k.clone().cross(this);
        return this.multiplyScalar(c).addScaledVector(x, s).addScaledVector(k, d * (1 - c));
    }
    /** The vector at index i of a flat array of triples. */
    from(array: ArrayLike<number>, i: number): this { return this.set(array[3 * i], array[3 * i + 1], array[3 * i + 2]); }
}

/** A linear RGB colour, as three.js's Color holds one. */
export class Colour {
    constructor(public r = 0, public g = 0, public b = 0) {}
    clone(): Colour { return new Colour(this.r, this.g, this.b); }
    lerp(c: Colour, t: number): this { this.r += (c.r - this.r) * t; this.g += (c.g - this.g) * t; this.b += (c.b - this.b) * t; return this; }
    multiplyScalar(s: number): this { this.r *= s; this.g *= s; this.b *= s; return this; }
}

export type Shade = Colour | ((p: V, n: V) => Colour);

export const v3 = (x = 0, y = 0, z = 0) => new V(x, y, z);
const linear = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
export const hex = (h: string) => new Colour(...([1, 3, 5].map(i => linear(parseInt(h.slice(i, i + 2), 16) / 255)) as Vec));
export const UP = v3(0, 0, 1);
export const GOLDEN = Math.PI * (3 - Math.sqrt(5));

/** Shade, body and lit-tip colour. */
export type Ramp = [string, string, string];
export const ramp = (r: Ramp) => r.map(hex) as [Colour, Colour, Colour];

export function smooth(a: number, b: number, x: number): number {
    const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
    return t * t * (3 - 2 * t);
}

/** A deterministic wobble, so hand-made lumps are the same on every load. */
export function noise(p: V, f: number, s: number): number {
    return Math.sin(p.x * f + s) * Math.sin(p.y * f * 1.31 + s * 2.1) * Math.sin(p.z * f * 0.87 + s * 0.7)
        + 0.5 * Math.sin(p.x * f * 2.13 - s) * Math.sin(p.z * f * 1.71 + s * 1.3);
}

/** A steady random value in [0, 1) per point. */
export function hash(p: V, seed: number): number {
    const h = Math.sin(p.x * 127.1 + p.y * 311.7 + p.z * 74.7 + seed * 17.3) * 43758.5453;
    return h - Math.floor(h);
}

/** Seeded random numbers in [0, 1), so scattered details are the same on every load. */
export function random(seed = 1): () => number {
    let s = (seed * 7919 + 1) % 2147483647 || 1;
    return () => (s = (s * 16807) % 2147483647) / 2147483647;
}

/** Triangles with a normal and a colour per vertex. */
export interface Geometry { position: number[]; normal: number[]; color: number[]; index: number[] }

const empty = (): Geometry => ({ position: [], normal: [], color: [], index: [] });
const count = (geo: Geometry) => geo.position.length / 3;

/** Area-weighted vertex normals of the triangles. */
export function computeNormals(geo: Geometry): Geometry {
    const n = new Float64Array(geo.position.length), a = v3(), b = v3(), c = v3(), ab = v3(), ac = v3();
    for (let i = 0; i < geo.index.length; i += 3) {
        const [ia, ib, ic] = [geo.index[i], geo.index[i + 1], geo.index[i + 2]];
        a.from(geo.position, ia);
        ab.subVectors(b.from(geo.position, ib), a);
        ac.subVectors(c.from(geo.position, ic), a);
        const f = ab.cross(ac);
        for (const k of [ia, ib, ic]) {
            n[3 * k] += f.x;
            n[3 * k + 1] += f.y;
            n[3 * k + 2] += f.z;
        }
    }
    geo.normal = new Array(geo.position.length);
    for (let k = 0; k < count(geo); k++) {
        const l = Math.hypot(n[3 * k], n[3 * k + 1], n[3 * k + 2]) || 1;
        for (let d = 0; d < 3; d++) geo.normal[3 * k + d] = n[3 * k + d] / l;
    }
    return geo;
}

/** Welds vertices at one position (to 1e-4), as three.js's mergeVertices does for positions alone. */
function weld(geo: Geometry): Geometry {
    const out = empty(), seen = new Map<string, number>(), map: number[] = [];
    for (let k = 0; k < count(geo); k++) {
        const name = `${Math.round(geo.position[3 * k] * 1e4)},${Math.round(geo.position[3 * k + 1] * 1e4)},${Math.round(geo.position[3 * k + 2] * 1e4)}`;
        let at = seen.get(name);
        if (at === undefined) {
            at = count(out);
            seen.set(name, at);
            out.position.push(geo.position[3 * k], geo.position[3 * k + 1], geo.position[3 * k + 2]);
            if (geo.color.length) out.color.push(geo.color[3 * k], geo.color[3 * k + 1], geo.color[3 * k + 2]);
        }
        map.push(at);
    }
    out.index = geo.index.map(i => map[i]);
    return out;
}

/**
 * Welds seams so normals stay smooth, then pushes the surface along its normals. Triangles that
 * weld to a line, like a lathe's rows at the axis, are dropped.
 */
export function soft(geometry: Geometry, amp = 0, freq = 3, seed = 0): Geometry {
    const geo = weld(geometry), kept: number[] = [];
    for (let i = 0; i < geo.index.length; i += 3) {
        const [a, b, c] = [geo.index[i], geo.index[i + 1], geo.index[i + 2]];
        if (a !== b && b !== c && a !== c) kept.push(a, b, c);
    }
    geo.index = kept;
    computeNormals(geo);
    if (!amp) return geo;
    const p = v3(), n = v3();
    for (let k = 0; k < count(geo); k++) {
        p.from(geo.position, k);
        p.addScaledVector(n.from(geo.normal, k), amp * noise(p, freq, seed));
        geo.position[3 * k] = p.x;
        geo.position[3 * k + 1] = p.y;
        geo.position[3 * k + 2] = p.z;
    }
    return computeNormals(geo);
}

/** Colours from a constant or from (position, normal) in the part's own frame. */
export function paint(geo: Geometry, shade: Shade): Geometry {
    const colour = typeof shade === 'function' ? shade : () => shade;
    const p = v3(), n = v3();
    geo.color = [];
    for (let k = 0; k < count(geo); k++) {
        const c = colour(p.from(geo.position, k), n.from(geo.normal, k));
        geo.color.push(c.r, c.g, c.b);
    }
    return geo;
}

/** A rotation as a 3 × 3 matrix, rows first. */
type Turn = number[];

/** The rotation that turns unit vector a onto unit vector b. */
export function turnFromTo(a: V, b: V): Turn {
    const axis = a.clone().cross(b), c = a.dot(b);
    if (axis.lengthSq() < 1e-12) {
        if (c > 0) return [1, 0, 0, 0, 1, 0, 0, 0, 1];
        // Half a turn about an axis normal to a.
        const k = (Math.abs(a.x) > 0.9 ? v3(0, 1, 0) : v3(1, 0, 0)).cross(a).normalize();
        return [2 * k.x * k.x - 1, 2 * k.x * k.y, 2 * k.x * k.z, 2 * k.x * k.y, 2 * k.y * k.y - 1, 2 * k.y * k.z, 2 * k.x * k.z, 2 * k.y * k.z, 2 * k.z * k.z - 1];
    }
    const s = axis.length(), k = axis.divideScalar(s), t = 1 - c;
    return [
        t * k.x * k.x + c, t * k.x * k.y - s * k.z, t * k.x * k.z + s * k.y,
        t * k.x * k.y + s * k.z, t * k.y * k.y + c, t * k.y * k.z - s * k.x,
        t * k.x * k.z - s * k.y, t * k.y * k.z + s * k.x, t * k.z * k.z + c,
    ];
}

/** The rotation of Euler angles in three.js's XYZ order: Rx · Ry · Rz. */
function euler([x, y, z]: Vec): Turn {
    const [a, b, c, d, e, f] = [Math.cos(x), Math.sin(x), Math.cos(y), Math.sin(y), Math.cos(z), Math.sin(z)];
    return [c * e, -c * f, d, a * f + b * e * d, a * e - b * f * d, -b * c, b * f - a * e * d, b * e + a * f * d, a * c];
}

interface Placement { at?: Vec | V; rot?: Vec; scale?: Vec; turn?: Turn }

/** Scales, turns and moves a part, with its normals. */
export function place(geo: Geometry, { at = [0, 0, 0], rot = [0, 0, 0], scale = [1, 1, 1], turn }: Placement = {}): Geometry {
    const m = turn ?? euler(rot), [tx, ty, tz] = at instanceof V ? at.toArray() : at;
    for (let k = 0; k < count(geo); k++) {
        const x = geo.position[3 * k] * scale[0], y = geo.position[3 * k + 1] * scale[1], z = geo.position[3 * k + 2] * scale[2];
        geo.position[3 * k] = m[0] * x + m[1] * y + m[2] * z + tx;
        geo.position[3 * k + 1] = m[3] * x + m[4] * y + m[5] * z + ty;
        geo.position[3 * k + 2] = m[6] * x + m[7] * y + m[8] * z + tz;
        if (!geo.normal.length) continue;
        // A normal turns with the inverse transpose: divided by the scale, then turned.
        const nx = geo.normal[3 * k] / scale[0], ny = geo.normal[3 * k + 1] / scale[1], nz = geo.normal[3 * k + 2] / scale[2];
        const n = v3(m[0] * nx + m[1] * ny + m[2] * nz, m[3] * nx + m[4] * ny + m[5] * nz, m[6] * nx + m[7] * ny + m[8] * nz).normalize();
        geo.normal[3 * k] = n.x;
        geo.normal[3 * k + 1] = n.y;
        geo.normal[3 * k + 2] = n.z;
    }
    return geo;
}

/** A round lump without poles, three.js's IcosahedronGeometry: 20 triangles at detail 0, 80 at 1, 180 at 2, 320 at 3. */
export function lump(r: number, detail: number): Geometry {
    const t = (1 + Math.sqrt(5)) / 2;
    const corners = [[-1, t, 0], [1, t, 0], [-1, -t, 0], [1, -t, 0], [0, -1, t], [0, 1, t], [0, -1, -t], [0, 1, -t], [t, 0, -1], [t, 0, 1], [-t, 0, -1], [-t, 0, 1]].map(([x, y, z]) => v3(x, y, z));
    const faces = [0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8, 3, 9, 4, 3, 4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1];
    const geo = empty(), cols = detail + 1;
    for (let f = 0; f < faces.length; f += 3) {
        const [a, b, c] = [corners[faces[f]], corners[faces[f + 1]], corners[faces[f + 2]]];
        const v: V[][] = [];
        for (let i = 0; i <= cols; i++) {
            const aj = a.clone().lerp(c, i / cols), bj = b.clone().lerp(c, i / cols), rows = cols - i;
            v.push(Array.from({ length: rows + 1 }, (_, j) => (j === 0 && i === cols ? aj : aj.clone().lerp(bj, j / rows))));
        }
        for (let i = 0; i < cols; i++) for (let j = 0; j < 2 * (cols - i) - 1; j++) {
            const k = Math.floor(j / 2);
            const tri = j % 2 === 0 ? [v[i][k + 1], v[i + 1][k], v[i][k]] : [v[i][k + 1], v[i + 1][k + 1], v[i + 1][k]];
            for (const p of tri) {
                geo.index.push(count(geo));
                geo.position.push(...p.clone().setLength(r).toArray());
            }
        }
    }
    return computeNormals(geo);
}

/** A surface of revolution about z from points (radius, z) bottom up, `segments` around, faces outward. */
export function revolve(points: [number, number][], segments: number): Geometry {
    const geo = empty(), n = points.length;
    for (let i = 0; i <= segments; i++) {
        const phi = (i / segments) * 2 * Math.PI;
        for (const [r, z] of points) geo.position.push(r * Math.sin(phi), -r * Math.cos(phi), z);
    }
    for (let i = 0; i < segments; i++) for (let j = 0; j < n - 1; j++) {
        const a = j + i * n, b = j + (i + 1) * n, c = j + 1 + (i + 1) * n, d = j + 1 + i * n;
        geo.index.push(a, b, d, c, d, b);
    }
    return computeNormals(geo);
}

/** A point of three.js's SplineCurve: Catmull-Rom through the points, `t` from 0 to 1 by parameter. */
function spline(points: [number, number][], t: number): [number, number] {
    const p = (points.length - 1) * t, i = Math.floor(p), w = p - i;
    const at = (k: number) => points[Math.min(points.length - 1, Math.max(0, k))];
    const [p0, p1, p2, p3] = [at(i === 0 ? i : i - 1), at(i), at(i + 1), at(i + 2)];
    const curve = (a: number, b: number, c: number, d: number) => {
        const v0 = (c - a) * 0.5, v1 = (d - b) * 0.5, t2 = w * w, t3 = w * t2;
        return (2 * b - 2 * c + v0 + v1) * t3 + (-3 * b + 3 * c - 2 * v0 - v1) * t2 + v0 * w + b;
    };
    return [curve(p0[0], p1[0], p2[0], p3[0]), curve(p0[1], p1[1], p2[1], p3[1])];
}

/**
 * A surface of revolution around z. The profile runs from the bottom of the axis outwards and up,
 * so faces point outwards; `rows` samples its spline.
 */
export function lathe(profile: [number, number][], segments = 32, rows = profile.length * 4): Geometry {
    const points = Array.from({ length: rows + 1 }, (_, k) => spline(profile, k / rows)).map(([x, z]): [number, number] => [Math.max(0, x), z]);
    return revolve(points, segments);
}

/** three.js's CapsuleGeometry along z: radius r, `length` between the caps' centres, tapering to radius `tip` at the top. */
function capsule(r: number, length: number, capSegments: number, sides: number, tip = r): Geometry {
    const points: [number, number][] = [];
    for (let k = 0; k <= capSegments; k++) {
        const a = -Math.PI / 2 + (k / capSegments) * (Math.PI / 2);
        points.push([r * Math.cos(a), -length / 2 + r * Math.sin(a)]);
    }
    for (let k = 0; k <= capSegments; k++) {
        const a = (k / capSegments) * (Math.PI / 2);
        points.push([tip * Math.cos(a), length / 2 + tip * Math.sin(a)]);
    }
    return revolve(points, sides);
}

/** A capsule from a to b, of radius r at a and `tip` at b, painted in the model's frame. */
export function limb(a: Vec, b: Vec, r: number, shade: Shade, amp = 0.01, sides = 12, tip = r): Geometry {
    const dir = v3(...b).sub(v3(...a));
    const geo = place(soft(capsule(r, dir.length(), Math.max(1, sides >> 2), sides, tip), amp, 8), {
        at: v3(...a).add(v3(...b)).multiplyScalar(0.5), turn: turnFromTo(UP, dir.normalize()),
    });
    return paint(geo, shade);
}

/** Normals out from a dome around `centre` with half-sizes `size`, leaning `lift` of the way up. */
export const domed = (centre: V, size: V, lift: number) => (p: V) => p.clone().sub(centre).divide(size).normalize().lerp(UP, lift).normalize();

/**
 * Loose triangles with their own colours. `light` gives each corner its normal, as `domed` does
 * so that many thin leaves light as one round crown; without it a face stays flat.
 */
export function sheet() {
    const geo = empty();
    return {
        add(corners: V[], colours: Colour[], light?: (p: V) => V) {
            const flat = v3().subVectors(corners[1], corners[0]).cross(v3().subVectors(corners[2], corners[0])).normalize();
            corners.forEach((p, i) => {
                const n = light ? light(p) : flat;
                geo.index.push(count(geo));
                geo.position.push(p.x, p.y, p.z);
                geo.normal.push(n.x, n.y, n.z);
                geo.color.push(colours[i].r, colours[i].g, colours[i].b);
            });
        },
        build: () => geo,
    };
}

export type Sheet = ReturnType<typeof sheet>;

/** Each triangle with its own three vertices, so a face can take its own colour. */
export function nonIndexed(geo: Geometry): Geometry {
    const out = empty();
    for (const i of geo.index) {
        out.index.push(count(out));
        out.position.push(geo.position[3 * i], geo.position[3 * i + 1], geo.position[3 * i + 2]);
        out.normal.push(geo.normal[3 * i], geo.normal[3 * i + 1], geo.normal[3 * i + 2]);
        out.color.push(geo.color[3 * i], geo.color[3 * i + 1], geo.color[3 * i + 2]);
    }
    return out;
}

/** Scales a model uniformly so its top is at z = 1. */
export function unit(geo: Geometry): Geometry {
    let top = -Infinity;
    for (let k = 2; k < geo.position.length; k += 3) top = Math.max(top, geo.position[k]);
    return place(geo, { scale: [1 / top, 1 / top, 1 / top] });
}

export function merge(parts: Geometry[]): Geometry {
    const out = empty();
    for (const part of parts) {
        const base = count(out);
        for (const v of part.position) out.position.push(v);
        for (const v of part.normal) out.normal.push(v);
        for (const v of part.color) out.color.push(v);
        for (const i of part.index) out.index.push(base + i);
    }
    return out;
}
