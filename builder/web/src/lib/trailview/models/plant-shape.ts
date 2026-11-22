// The parts that the plant models build from, on top of shape.ts.

import { computeNormals, GOLDEN, paint, place, revolve, soft, smooth, UP, v3, type Colour, type Geometry, type Sheet, type V, type Vec } from './shape';

export * from './shape';

/** Darker at the root, lit at the tip, by height. */
export const byHeight = ([deep, mid, tip]: Colour[], top: number) => (p: V) =>
    deep.clone().lerp(mid, smooth(0, 0.4 * top, p.z)).lerp(tip, smooth(0.5 * top, top, p.z));

/** A ramp's colour at `t`, clamped to [0, 1]: the shade at 0, the body at 0.5, the tip at 1. */
export function rampAt([shade, body, tip]: Colour[], t: number): Colour {
    const u = Math.min(1, Math.max(0, t));
    return u < 0.5 ? shade.clone().lerp(body, u * 2) : body.clone().lerp(tip, u * 2 - 1);
}

/** three.js's CatmullRomCurve3, centripetal, open: points by arc length with `at`. */
function curve(points: V[]) {
    const n = points.length;
    const point = (t: number): V => {
        const p = (n - 1) * t;
        let i = Math.floor(p), w = p - i;
        if (i >= n - 1) {
            i = n - 2;
            w = 1;
        }
        const p1 = points[i], p2 = points[i + 1];
        const p0 = i > 0 ? points[i - 1] : p1.clone().multiplyScalar(2).sub(p2);
        const p3 = i + 2 < n ? points[i + 2] : p2.clone().multiplyScalar(2).sub(p1);
        let dt0 = p0.distanceTo(p1) ** 0.5, dt1 = p1.distanceTo(p2) ** 0.5, dt2 = p2.distanceTo(p3) ** 0.5;
        if (dt1 < 1e-4) dt1 = 1;
        if (dt0 < 1e-4) dt0 = dt1;
        if (dt2 < 1e-4) dt2 = dt1;
        const axis = (a: number, b: number, c: number, d: number) => {
            let t1 = (b - a) / dt0 - (c - a) / (dt0 + dt1) + (c - b) / dt1;
            let t2 = (c - b) / dt1 - (d - b) / (dt1 + dt2) + (d - c) / dt2;
            t1 *= dt1;
            t2 *= dt1;
            const c2 = -3 * b + 3 * c - 2 * t1 - t2, c3 = 2 * b - 2 * c + t1 + t2;
            return b + t1 * w + c2 * w * w + c3 * w * w * w;
        };
        return v3(axis(p0.x, p1.x, p2.x, p3.x), axis(p0.y, p1.y, p2.y, p3.y), axis(p0.z, p1.z, p2.z, p3.z));
    };
    // The arc length by 200 divisions, as three.js measures it.
    const lengths = [0];
    let last = point(0);
    for (let k = 1; k <= 200; k++) {
        const next = point(k / 200);
        lengths.push(lengths[k - 1] + next.distanceTo(last));
        last = next;
    }
    const at = (u: number): V => {
        const target = u * lengths[200];
        let k = 0;
        while (k < 199 && lengths[k + 1] < target) k++;
        const span = lengths[k + 1] - lengths[k];
        return point((k + (span > 0 ? (target - lengths[k]) / span : 0)) / 200);
    };
    const tangent = (u: number): V => {
        const a = Math.max(0, u - 1e-4), b = Math.min(1, u + 1e-4);
        return at(b).sub(at(a)).normalize();
    };
    return { at, tangent };
}

/** A tube along a smooth path, its radius shrinking linearly to `tip` times `r` at the end. */
export function tube(points: Vec[], r: number, tip = 1, segments = 16, sides = 8): Geometry {
    const path = curve(points.map(p => v3(...p)));
    const tangents = Array.from({ length: segments + 1 }, (_, i) => path.tangent(i / segments));
    // Frames by parallel transport, as three.js's computeFrenetFrames.
    const t0 = tangents[0], ax = Math.abs(t0.x), ay = Math.abs(t0.y), az = Math.abs(t0.z);
    const least = ax <= ay && ax <= az ? v3(1, 0, 0) : ay <= az ? v3(0, 1, 0) : v3(0, 0, 1);
    const normals = [t0.clone().cross(t0.clone().cross(least).normalize())];
    for (let i = 1; i <= segments; i++) {
        const n = normals[i - 1].clone(), axis = tangents[i - 1].clone().cross(tangents[i]);
        if (axis.length() > Number.EPSILON) n.applyAxisAngle(axis.normalize(), Math.acos(Math.min(1, Math.max(-1, tangents[i - 1].dot(tangents[i])))));
        normals.push(n);
    }
    const geo: Geometry = { position: [], normal: [], color: [], index: [] };
    for (let i = 0; i <= segments; i++) {
        const c = path.at(i / segments), n = normals[i], b = tangents[i].clone().cross(n), k = 1 + (tip - 1) * (i / segments);
        for (let j = 0; j <= sides; j++) {
            const v = (j / sides) * Math.PI * 2, d = n.clone().multiplyScalar(-Math.cos(v)).addScaledVector(b, Math.sin(v)).normalize();
            geo.position.push(...c.clone().addScaledVector(d, r * k).toArray());
            geo.normal.push(...d.toArray());
        }
    }
    for (let j = 1; j <= segments; j++) for (let i = 1; i <= sides; i++) {
        const a = (sides + 1) * (j - 1) + (i - 1), b = (sides + 1) * j + (i - 1), c = (sides + 1) * j + i, d = (sides + 1) * (j - 1) + i;
        geo.index.push(a, b, d, b, c, d);
    }
    return geo;
}

/**
 * A flat strip along a smooth path, `width(t)` wide across `side`, for blades and fronds. Its
 * normals lean `lift` of the way to up, so thin plants light like the ground they stand on.
 */
export function ribbon(points: Vec[], side: Vec, width: (t: number) => number, segments: number, lift = 0.5): Geometry {
    const path = curve(points.map(p => v3(...p))), across = v3(...side).normalize();
    const geo: Geometry = { position: [], normal: [], color: [], index: [] };
    for (let i = 0; i <= segments; i++) {
        const t = i / segments, c = path.at(t), w = width(t) / 2;
        geo.position.push(...c.clone().addScaledVector(across, -w).toArray(), ...c.clone().addScaledVector(across, w).toArray());
    }
    for (let i = 0; i < segments; i++) geo.index.push(2 * i, 2 * i + 1, 2 * i + 3, 2 * i, 2 * i + 3, 2 * i + 2);
    const strip = soft(geo);
    for (let k = 0; k < strip.normal.length; k += 3) {
        const n = v3(strip.normal[k], strip.normal[k + 1], strip.normal[k + 2]);
        if (n.z < 0) n.negate();
        strip.normal.splice(k, 3, ...n.lerp(UP, lift).normalize().toArray());
    }
    return strip;
}

/**
 * A leaf or blade from `base` towards angle `a`. It leaves at `tilt` from vertical, and its tip
 * drops `arch` lengths below that line.
 */
export function blade(base: Vec, a: number, length: number, tilt: number, arch: number, width: (t: number) => number, segments: number): Geometry {
    const out: Vec = [Math.cos(a), Math.sin(a), 0];
    const along = (s: number): Vec => [
        base[0] + out[0] * Math.sin(tilt) * s * length,
        base[1] + out[1] * Math.sin(tilt) * s * length,
        base[2] + (Math.cos(tilt) * s - arch * s * s) * length,
    ];
    return ribbon([base, along(0.5), along(1)], [-out[1], out[0], 0], width, segments);
}

/** Sets each normal to the one `light` gives its vertex, so many small parts light as one round bush. */
export function relight(geo: Geometry, light: (p: V) => V): Geometry {
    for (let k = 0; k < geo.position.length; k += 3) geo.normal.splice(k, 3, ...light(v3(geo.position[k], geo.position[k + 1], geo.position[k + 2])).toArray());
    return geo;
}

/** A tuft of a cushion: centre x and y, radius and height. */
export type Tuft = [number, number, number, number];

/**
 * `middle` and `count` lower tufts round it, each `d` out, `r` wide and `h` tall plus up to
 * `[dd, dr, dh]` more, and up to `turn` radians past its even place in the ring.
 */
export function tufts(middle: Tuft, count: number, turn: number, [d, r, h]: Vec, [dd, dr, dh]: Vec, rand: () => number): Tuft[] {
    const out = [middle];
    for (let k = 0; k < count; k++) {
        const a = (k / count) * 2 * Math.PI + rand() * turn;
        const at = d + dd * rand();
        out.push([at * Math.cos(a), at * Math.sin(a), r + dr * rand(), h + dh * rand()]);
    }
    return out;
}

/**
 * A fern's feather on `cards` along `path`, level across `side`: `pairs` broad teeth on each flank,
 * those of one flank half a step past those of the other, so the feather is whole along its length,
 * with a ridge. A tooth `t` along it reaches `reach(t)` out and drops `droop` of that; `colour(s)`
 * along the ridge, its point turned toward `tip`. The point stands on the chord of its step, or
 * with `onPath` on the path; `under` lights the face that looks down. 2 × `pairs` triangles.
 */
export function feather(cards: Sheet, path: (s: number) => V, side: V, pairs: number, reach: (t: number) => number, droop: number, colour: (s: number) => Colour, tip: Colour, { onPath = false, under = false } = {}): void {
    for (const flank of [-1, 1]) for (let i = 0; i < pairs; i++) {
        const [s0, s1] = [i, i + 1].map((k) => (k + (flank > 0 ? 0.5 : 0)) / (pairs + 0.5)), t = (s0 + s1) / 2, r = reach(t);
        const [from, to] = [path(s0), path(s1)];
        const up = v3().crossVectors(side, to.clone().sub(from)).normalize();
        if (up.z < 0 !== under) up.negate();
        const end = (onPath ? path(t + 0.2 * (s1 - s0)) : from.clone().lerp(to, 0.75)).addScaledVector(side, flank * r).addScaledVector(up, -droop * r);
        end.z = Math.max(0.01, end.z);
        const light = (p: V) => (p === end ? up.clone().addScaledVector(side, 0.6 * flank) : up.clone()).normalize().lerp(UP, 0.3).normalize();
        cards.add([from, to, end], [colour(s0), colour(s1), colour(t).lerp(tip, 0.6)], light);
    }
}

/** A blade on `cards`: a kite from `foot` to `tip`, twice `half` across at its `waist`. 2 triangles. */
export function kite(cards: Sheet, foot: V, waist: V, tip: V, half: V, [root, body, end]: Colour[], light?: (p: V) => V): void {
    const [left, right] = [waist.clone().add(half), waist.clone().sub(half)];
    cards.add([foot, left, right], [root, body, body], light);
    cards.add([left, tip, right], [body, end, body], light);
}

/** A leaf on `cards`: a kite from `at` along the unit `dir`, `length` long and `width` across at `waist` of its length, level across. 2 triangles. */
export function leaf(cards: Sheet, at: V, dir: V, length: number, width: number, waist: number, colours: Colour[], light?: (p: V) => V): void {
    const half = v3().crossVectors(dir, UP);
    if (half.lengthSq() < 1e-6) half.set(1, 0, 0);
    kite(cards, at, at.clone().addScaledVector(dir, waist * length), at.clone().addScaledVector(dir, length), half.setLength(width / 2), colours, light);
}

/** Two unit vectors across the unit `axis`, square to each other. */
export function across(axis: V): [V, V] {
    const u = v3().crossVectors(axis, Math.abs(axis.z) < 0.9 ? UP : v3(1, 0, 0)).normalize();
    return [u, v3().crossVectors(axis, u)];
}

/**
 * A stem, twig, spine or spray on `cards`: a three-sided point from `foot`, `r` thick there, to `tip`, `root` at its
 * foot and `end` at its tip. 3 triangles, lit as a round stick that leans the way `lean` gives, so sprays on a dome
 * shade with it.
 */
export function spike(cards: Sheet, foot: V, tip: V, r: number, [root, end]: Colour[], lean: (p: V) => V = () => UP): void {
    const axis = tip.clone().sub(foot).normalize(), [u, w] = across(axis);
    const ring = [0, 1, 2, 0].map((k) => foot.clone().addScaledVector(u, r * Math.cos((k * 2 * Math.PI) / 3)).addScaledVector(w, r * Math.sin((k * 2 * Math.PI) / 3)));
    const light = (p: V) => p.clone().sub(foot).projectOnPlane(axis).normalize().addScaledVector(axis, 0.3).add(lean(p)).normalize();
    for (let k = 0; k < 3; k++) cards.add([ring[k], ring[k + 1], tip], [root, root, end], light);
}

/**
 * The place on a tuft's dome toward angle `a`, `polar` radians down from its top: the point, a little inside the
 * skin of a `cushion` so that what grows there joins it, and the way out.
 */
export function domeAt([x, y, r, h]: Tuft, a: number, polar: number): [V, V] {
    const out = v3(Math.sin(polar) * Math.cos(a), Math.sin(polar) * Math.sin(a), Math.cos(polar));
    return [v3(x + 0.88 * r * out.x, y + 0.88 * r * out.y, 0.88 * h * out.z), out];
}

/** Place `j` of `n` on a tuft's dome, spread by the golden angle from its top down to `reach` radians and turned by `turn`. */
export const onDome = (tuft: Tuft, j: number, n: number, reach: number, turn = 0) => domeAt(tuft, turn + j * GOLDEN, reach * Math.sqrt((j + 0.5) / n));

/**
 * A seed head, plume or bud on `cards`: a spindle of `sides` sides from `base` on its stalk to `tip`, `r` at its
 * widest, `at` of the way up, `low` at its base, `high` from there on and `end` at its tip. 2 × `sides` triangles, lit as a round body
 * that leans up.
 */
export function spindle(cards: Sheet, base: V, tip: V, r: number, sides: number, [low, high, end = high]: Colour[], at = 0.4): void {
    const axis = tip.clone().sub(base).normalize(), waist = base.clone().lerp(tip, at), [u, w] = across(axis);
    const ring = (k: number) => waist.clone().addScaledVector(u, r * Math.cos((k / sides) * 2 * Math.PI)).addScaledVector(w, r * Math.sin((k / sides) * 2 * Math.PI));
    const light = (p: V) => p.clone().sub(waist).normalize().lerp(UP, 0.4).normalize();
    for (let k = 0; k < sides; k++) {
        cards.add([base, ring(k), ring(k + 1)], [low, high, high], light);
        cards.add([ring(k), tip, ring(k + 1)], [high, end, high], light);
    }
}

/**
 * A round cushion of a mat, sunk a little into the ground: a dome `sides` round, of 3 × `sides` triangles, its rim
 * knobbly, `deep` in the crease at its rim and `tip` on its top. Its light leans `lift` of the way up, so cushions
 * that grow into each other shade softly into one mat.
 */
export function cushion([x, y, r, h]: Tuft, sides: number, [deep, mid, tip]: Colour[], lift: number, seed: number): Geometry {
    const dome = soft(revolve([[r, 0], [0.74 * r, 0.62 * h], [0, h]], sides), 0.1 * r, 5 / r, seed);
    relight(dome, (p) => v3(p.x / r, p.y / r, p.z / h).normalize().lerp(UP, lift).normalize());
    place(dome, { at: [x, y, -0.12 * h], rot: [0, 0, seed] });
    return paint(dome, (p) => deep.clone().lerp(mid, smooth(-0.12, 0.4, p.z / h)).lerp(tip, smooth(0.35, 0.85, p.z / h)));
}

/** A solid of three.js's PolyhedronGeometry at detail 0: its corners on a sphere of radius r, flat faces. */
function polyhedron(corners: number[], faces: number[], r: number): Geometry {
    const geo: Geometry = { position: [], normal: [], color: [], index: [] };
    for (const i of faces) {
        geo.index.push(geo.index.length);
        geo.position.push(...v3(corners[3 * i], corners[3 * i + 1], corners[3 * i + 2]).setLength(r).toArray());
    }
    return computeNormals(geo);
}

export const tetrahedron = (r: number) => polyhedron([1, 1, 1, -1, -1, 1, -1, 1, -1, 1, -1, -1], [2, 1, 0, 0, 3, 2, 1, 3, 0, 2, 3, 1], r);
export const octahedron = (r: number) => polyhedron([1, 0, 0, -1, 0, 0, 0, 1, 0, 0, -1, 0, 0, 0, 1, 0, 0, -1], [0, 2, 4, 0, 4, 3, 0, 3, 5, 0, 5, 2, 1, 2, 5, 1, 5, 3, 1, 3, 4, 1, 4, 2], r);
