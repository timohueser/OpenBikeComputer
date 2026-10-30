// PROTOTYPE road system (a study; never merges). Where water lies beside a corridor: the metres from a station
// along a ray across it to the edge of a river's or a stream's water, a lake or a river area, so that the sides
// of a way stop at the water's edge (corridor.ts). A river or a stream is the capsules of its half-width about
// its segments, as its ribbon and the stamp's core hold it but for the mitres of its bends.
import { insideRings, metresPerUnit, RIVER, STREAM, type Line } from '../terrain/bins';

/**
 * The first metres along the ray from world point (x, y) in direction (dx, dy), world units per metre, between
 * `from` and `to` metres where water lies; `to` where none does.
 */
export type Shore = (x: number, y: number, dx: number, dy: number, from: number, to: number) => number;

/** The cells of the index, in metres. */
const CELL_M = 32;

/** A capsule about a waterway's segment, or a ring's edge of a lake or a river area, in world units. */
interface Piece { ax: number; ay: number; bx: number; by: number; r?: number; rings?: number[][] }

/** The shore of a tile's waterways and the rings of its lakes and river areas; undefined without water. */
export function shoreOf(lines: Line[], areas: { rings: number[][] }[]): Shore | undefined {
    const pieces: Piece[] = [];
    for (const line of lines) {
        if (line.kind !== RIVER && line.kind !== STREAM) continue;
        for (let v = 0; v + 1 < line.z.length; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4);
            pieces.push({ ax, ay, bx, by, r: line.h / metresPerUnit(ay) });
        }
    }
    for (const { rings } of areas) {
        for (const ring of rings) for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) pieces.push({ ax: ring[j], ay: ring[j + 1], bx: ring[i], by: ring[i + 1], rings });
    }
    if (!pieces.length) return undefined;
    const size = CELL_M / metresPerUnit(pieces[0].ay), cells = new Map<string, Piece[]>();
    const each = (x0: number, y0: number, x1: number, y1: number, visit: (key: string) => void) => {
        for (let j = Math.floor(y0 / size); j <= Math.floor(y1 / size); j++) for (let i = Math.floor(x0 / size); i <= Math.floor(x1 / size); i++) visit(`${i},${j}`);
    };
    for (const p of pieces) {
        const r = p.r ?? 0;
        each(Math.min(p.ax, p.bx) - r, Math.min(p.ay, p.by) - r, Math.max(p.ax, p.bx) + r, Math.max(p.ay, p.by) + r, key => (cells.get(key) ?? cells.set(key, []).get(key)!).push(p));
    }
    return (x, y, dx, dy, from, to) => {
        if (to <= from) return to;
        const px = x + dx * from, py = y + dy * from, vx = dx * (to - from), vy = dy * (to - from);
        const near = new Set<Piece>();
        each(Math.min(px, px + vx), Math.min(py, py + vy), Math.max(px, px + vx), Math.max(py, py + vy), key => cells.get(key)?.forEach(p => near.add(p)));
        let first = 1;
        const tested = new Set<number[][]>();
        for (const p of near) {
            if (p.r !== undefined) first = Math.min(first, capsuleEntry(px, py, vx, vy, p));
            else {
                if (!tested.has(p.rings!)) {
                    tested.add(p.rings!);
                    if (insideRings(p.rings!, px, py)) return from;
                }
                first = Math.min(first, edgeCrossing(px, py, vx, vy, p));
            }
        }
        return from + first * (to - from);
    };
}

/** The first share of the way p + t·v, t from 0 to 1, inside a capsule; 1 where it stays outside. */
function capsuleEntry(px: number, py: number, vx: number, vy: number, { ax, ay, bx, by, r }: Piece): number {
    const rr = r!, ex = bx - ax, ey = by - ay, l = Math.hypot(ex, ey);
    let first = Math.min(discEntry(px - ax, py - ay, vx, vy, rr), discEntry(px - bx, py - by, vx, vy, rr));
    if (l > 0) {
        // The rectangle between the discs, its half-planes clipping the way (Liang–Barsky).
        const ux = ex / l, uy = ey / l, qx = px - ax, qy = py - ay;
        let t0 = 0, t1 = 1;
        for (const [a, b] of [[-(qx * ux + qy * uy), -(vx * ux + vy * uy)], [qx * ux + qy * uy - l, vx * ux + vy * uy], [-(qx * -uy + qy * ux) - rr, -(vx * -uy + vy * ux)], [qx * -uy + qy * ux - rr, vx * -uy + vy * ux]]) {
            // a + b·t <= 0 inside.
            if (Math.abs(b) < 1e-12) {
                if (a > 0) t0 = 2;
            } else if (b > 0) t1 = Math.min(t1, -a / b);
            else t0 = Math.max(t0, -a / b);
        }
        if (t0 <= t1) first = Math.min(first, t0);
    }
    return first;
}

/** The first share of the way q + t·v (q about the disc's centre) inside a disc of radius r; 1 where it stays outside. */
function discEntry(qx: number, qy: number, vx: number, vy: number, r: number): number {
    const a = vx * vx + vy * vy, b = qx * vx + qy * vy, c = qx * qx + qy * qy - r * r;
    if (c <= 0) return 0;
    const d = b * b - a * c;
    if (d < 0 || b > 0 || a === 0) return 1;
    return Math.min(1, (-b - Math.sqrt(d)) / a);
}

/** The share of the way p + t·v where it first crosses an edge; 1 where it does not. */
function edgeCrossing(px: number, py: number, vx: number, vy: number, { ax, ay, bx, by }: Piece): number {
    const ex = bx - ax, ey = by - ay, den = vx * ey - vy * ex;
    if (Math.abs(den) < 1e-12) return 1;
    const t = ((ax - px) * ey - (ay - py) * ex) / den, s = ((ax - px) * vy - (ay - py) * vx) / den;
    return t >= 0 && t <= 1 && s >= 0 && s <= 1 ? t : 1;
}
