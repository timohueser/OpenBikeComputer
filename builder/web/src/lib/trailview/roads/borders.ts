// The borders of a piece's cover, for the cover pass of cover.wgsl: the parts of its covering faces' edges in plan where
// the faces may end. A texel whose centre a face covers and that no border reaches lies whole in the faces, also where
// two faces leave a crack narrower than a texel between them.
import { CF, MAT } from './section';

/** Whether a face of look word `word` covers the ground under it, as cover.wgsl: a detail's or a deck's faces do not. */
export const covers = (word: number) => (word & 127) < MAT.RAIL && ((word >>> 11) & CF.DECK) === 0;

/** Steps of plan per index cell of the faces, about a metre, and the cells from a piece's middle to the edge of the steps that its vertices reach. */
const CELL = 512, HALF = 2 ** 17 / CELL;
/** No edge's key in the table of `borders`: no two points have the ids 2^16 - 1. */
const EMPTY = 0xffffffff;
/** A span of an edge shorter than this, in steps, between two faces' insides or at an end, is no border: the rounding of where two faces meet. */
const TOUCH = 0.01;

/**
 * The borders of the covering faces among the triangles of `indices`, whose vertices lie at whole steps `xy` (x, y per
 * vertex) and have look words `words`: per border its two end points, in steps. A point where the cover ends lies on an
 * edge whose faces on its two sides do not pair up, and in no face's inside. So the borders are those edges, less the
 * spans where opposite edges on one line pair up and the spans inside a face. Where the cover ends at a point left out
 * so, a border passes within TOUCH: a hole that no border marks is narrower than TOUCH.
 */
export function borders(xy: Int32Array, words: Uint32Array, indices: ArrayLike<number>): Int32Array {
    // Faces as a, b, c counter-clockwise in plan; a face with no area covers nothing.
    const faces = new Int32Array(2 * indices.length), corners = new Int32Array(indices.length);
    let n = 0;
    for (let t = 0; t + 2 < indices.length; t += 3) {
        const i = indices[t];
        if (!covers(words[i])) continue;
        let j = indices[t + 1], k = indices[t + 2];
        const cross = (xy[2 * j] - xy[2 * i]) * (xy[2 * k + 1] - xy[2 * i + 1]) - (xy[2 * k] - xy[2 * i]) * (xy[2 * j + 1] - xy[2 * i + 1]);
        if (cross === 0) continue;
        if (cross < 0) [j, k] = [k, j];
        corners[n] = i;
        corners[n + 1] = j;
        corners[n + 2] = k;
        for (let c = n; c < n + 3; c++) {
            faces[2 * c] = xy[2 * corners[c]];
            faces[2 * c + 1] = xy[2 * corners[c] + 1];
        }
        n += 3;
    }
    // Vertices at one plan point are one point, in a table open by address.
    const table = 2 ** Math.ceil(Math.log2(xy.length + 2)), tx = new Int32Array(table), ty = new Int32Array(table), tid = new Int32Array(table).fill(-1);
    const points: number[] = [], pointOf = new Int32Array(xy.length / 2).fill(-1);
    for (let c = 0; c < n; c++) {
        const v = corners[c], x = xy[2 * v], y = xy[2 * v + 1];
        if (pointOf[v] >= 0) continue;
        let slot = ((Math.imul(x, 0x9e3779b1) ^ Math.imul(y, 0x85ebca6b)) >>> 0) & (table - 1);
        while (tid[slot] >= 0 && (tx[slot] !== x || ty[slot] !== y)) slot = (slot + 1) & (table - 1);
        if (tid[slot] < 0) {
            [tx[slot], ty[slot], tid[slot]] = [x, y, points.length / 2];
            points.push(x, y);
        }
        pointOf[v] = tid[slot];
    }
    // Per edge from its lesser point to the greater, the faces on its left less those on its right, in a table open by
    // address: the key of an edge is the lesser point's id times 2^16 and the greater's.
    const size = 2 ** Math.ceil(Math.log2(2 * n + 2)), keys = new Uint32Array(size).fill(EMPTY), counts = new Int32Array(size);
    for (let f = 0; f < n; f += 3) {
        for (let e = 0; e < 3; e++) {
            const p = pointOf[corners[f + e]], q = pointOf[corners[f + (e + 1) % 3]], key = (p < q ? p * 2 ** 16 + q : q * 2 ** 16 + p) >>> 0;
            let slot = (Math.imul(key, 0x9e3779b1) >>> 0) & (size - 1);
            while (keys[slot] !== EMPTY && keys[slot] !== key) slot = (slot + 1) & (size - 1);
            keys[slot] = key;
            counts[slot] += p < q ? 1 : -1;
        }
    }
    // The edges that do not pair up, by their line: its direction, whole and least, and the cross product of a point with it.
    const lines = new Map<string, { a: number[]; b: number[]; n: number }[]>();
    for (let slot = 0; slot < size; slot++) {
        if (keys[slot] === EMPTY || counts[slot] === 0) continue;
        const p = Math.floor(keys[slot] / 2 ** 16), q = keys[slot] % 2 ** 16, a = [points[2 * p], points[2 * p + 1]], b = [points[2 * q], points[2 * q + 1]];
        const g = gcd(Math.abs(b[0] - a[0]), Math.abs(b[1] - a[1])), d = [(b[0] - a[0]) / g, (b[1] - a[1]) / g];
        const [dx, dy] = d[0] > 0 || (d[0] === 0 && d[1] > 0) ? d : [-d[0], -d[1]], line = `${dx},${dy},${dx * a[1] - dy * a[0]}`;
        (lines.get(line) ?? lines.set(line, []).get(line)!).push({ a, b, n: counts[slot] });
    }
    // Along each line, the spans where the counts do not add up to nothing.
    const open: number[] = [];
    for (const edges of lines.values()) {
        if (edges.length === 1) {
            open.push(...edges[0].a, ...edges[0].b);
            continue;
        }
        const [dx, dy] = [edges[0].b[0] - edges[0].a[0], edges[0].b[1] - edges[0].a[1]], along = ([x, y]: number[]) => x * dx + y * dy;
        const events = edges.flatMap(({ a, b, n }) => [{ t: along(a), p: a, n }, { t: along(b), p: b, n: -n }]).sort((p, q) => p.t - q.t);
        let sum = 0;
        for (let i = 0; i + 1 < events.length; i++) {
            sum += events[i].n;
            if (sum !== 0 && events[i + 1].t > events[i].t) open.push(...events[i].p, ...events[i + 1].p);
        }
    }
    // The faces by the index cells that the open spans reach, as lists through `entries` (face, next); then each open span
    // less its parts inside a face.
    const cellOf = (c: number) => Math.min(2 * HALF - 1, Math.max(0, Math.floor(c / CELL) + HALF));
    const reached = new Uint8Array(4 * HALF * HALF), head = new Int32Array(4 * HALF * HALF).fill(-1), entries: number[] = [];
    for (let s = 0; s < open.length; s += 4) {
        for (let i = cellOf(Math.min(open[s], open[s + 2])); i <= cellOf(Math.max(open[s], open[s + 2])); i++) {
            for (let j = cellOf(Math.min(open[s + 1], open[s + 3])); j <= cellOf(Math.max(open[s + 1], open[s + 3])); j++) reached[i * 2 * HALF + j] = 1;
        }
    }
    for (let f = 0; f < 2 * n; f += 6) {
        const i0 = cellOf(Math.min(faces[f], faces[f + 2], faces[f + 4])), i1 = cellOf(Math.max(faces[f], faces[f + 2], faces[f + 4]));
        const j0 = cellOf(Math.min(faces[f + 1], faces[f + 3], faces[f + 5])), j1 = cellOf(Math.max(faces[f + 1], faces[f + 3], faces[f + 5]));
        for (let i = i0; i <= i1; i++) {
            for (let j = j0; j <= j1; j++) {
                const c = i * 2 * HALF + j;
                if (!reached[c]) continue;
                entries.push(f, head[c]);
                head[c] = entries.length / 2 - 1;
            }
        }
    }
    const out: number[] = [], seen = new Int32Array(n / 3).fill(-1), spans: number[] = [];
    for (let s = 0; s < open.length; s += 4) {
        const ax = open[s], ay = open[s + 1], bx = open[s + 2], by = open[s + 3], length = Math.hypot(bx - ax, by - ay);
        spans.length = 0;
        for (let i = cellOf(Math.min(ax, bx)); i <= cellOf(Math.max(ax, bx)); i++) {
            for (let j = cellOf(Math.min(ay, by)); j <= cellOf(Math.max(ay, by)); j++) {
                for (let e = head[i * 2 * HALF + j]; e >= 0; e = entries[2 * e + 1]) {
                    const f = entries[2 * e];
                    if (seen[f / 6] === s) continue;
                    seen[f / 6] = s;
                    inside(faces, f, ax, ay, bx, by, spans);
                }
            }
        }
        const order = Array.from({ length: spans.length / 2 }, (_, k) => k).sort((p, q) => spans[2 * p] - spans[2 * q]);
        const keep = (from: number, to: number) => {
            if ((to - from) * length >= TOUCH) out.push(Math.round(ax + (bx - ax) * from), Math.round(ay + (by - ay) * from), Math.round(ax + (bx - ax) * to), Math.round(ay + (by - ay) * to));
        };
        let at = 0;
        for (const k of order) {
            keep(at, spans[2 * k]);
            at = Math.max(at, spans[2 * k + 1]);
        }
        keep(at, 1);
    }
    return Int32Array.from(out);
}

/** Adds to `spans` the span (t0, t1) of the points a + t (b - a) that lie inside face f, open at both ends, if there is one. */
function inside(faces: Int32Array, f: number, ax: number, ay: number, bx: number, by: number, spans: number[]): void {
    let lo = 0, hi = 1;
    for (let e = 0; e < 3; e++) {
        const px = faces[f + 2 * e], py = faces[f + 2 * e + 1], qx = faces[f + (2 * e + 2) % 6], qy = faces[f + (2 * e + 3) % 6];
        // Inside where (q - p) × (x - p) > 0, x = a + t (b - a): c0 + t c1.
        const c0 = (qx - px) * (ay - py) - (qy - py) * (ax - px), c1 = (qx - px) * (by - ay) - (qy - py) * (bx - ax);
        if (c1 === 0) {
            if (c0 <= 0) return;
        } else if (c1 > 0) lo = Math.max(lo, -c0 / c1);
        else hi = Math.min(hi, -c0 / c1);
        if (lo >= hi) return;
    }
    spans.push(lo, hi);
}

function gcd(a: number, b: number): number {
    while (b) [a, b] = [b, a % b];
    return a;
}
