// The carriageways of a tile, in a grid: a band's row over another way sinks under it, so that way draws over it.
import { metresPerUnit, shoulderOf, type Line } from '../terrain/bins';
import { pivotsOf } from '../terrain/pivots';
import { along, gridOf, near, type Grid } from '../terrain/town';
import { isDeck } from './decks';
import { corridorLine } from './section';

/** A point this far inside a carriageway's edge lies on it. */
const INSIDE_M = 0.05;

/** A segment of a carriageway with its half-width, or a quad of a tight bend's fill between two of its cross-sections. */
export interface Ways extends Grid { segments: { xy: number[]; h: number; quad?: number[] }[] }

/** The carriageways of lines; with `half`, the half-width at a line's vertex, also the fills of their tight bends (terrain/pivots.ts). */
export function waysOf(lines: Line[], half?: (line: Line, v: number) => number): Ways {
    const segments: Ways['segments'] = [];
    // A deck stands over the bands under it.
    for (const line of lines.filter(way => corridorLine(way) && !isDeck(way))) {
        for (let v = 0; v + 1 < line.z.length; v++) segments.push({ xy: line.xy.slice(2 * v, 2 * v + 4), h: line.h + (line.plan?.widen[v] ?? 0) });
        if (!half) continue;
        for (const { x, y, r, first, last } of pivotsOf(line, v => half(line, v))) {
            // The carriageway's edge on a rung: its share of the core, which reaches the arc (stations.ts rungsOf).
            const end = (v: number) => {
                const [cx, cy] = line.xy.slice(2 * v, 2 * v + 2), d = Math.hypot(x - cx, y - cy) * metresPerUnit(cy), h = half(line, v), core = h + shoulderOf(line);
                const l = (h * Math.max(core, d - r)) / core / d;
                return [cx + (x - cx) * l, cy + (y - cy) * l];
            };
            for (let v = first; v < last; v++) segments.push({ xy: line.xy.slice(2 * v, 2 * v + 4), h: 0, quad: [...line.xy.slice(2 * v, 2 * v + 2), ...end(v), ...end(v + 1), ...line.xy.slice(2 * v + 2, 2 * v + 4)] });
        }
    }
    const boxes = segments.map(({ xy: [ax, ay, bx, by], h, quad }) => {
        if (quad) return [Math.min(...quad.filter((_, i) => i % 2 === 0)), Math.min(...quad.filter((_, i) => i % 2 === 1)), Math.max(...quad.filter((_, i) => i % 2 === 0)), Math.max(...quad.filter((_, i) => i % 2 === 1))];
        const r = h / metresPerUnit(ay);
        return [Math.min(ax, bx) - r, Math.min(ay, by) - r, Math.max(ax, bx) + r, Math.max(ay, by) + r];
    });
    return { ...gridOf(boxes), segments };
}

/** Whether point x, y lies in triangle a, b, c of world points. */
function inTriangle(x: number, y: number, [ax, ay, bx, by, cx, cy]: number[]): boolean {
    const d = [(bx - ax) * (y - ay) - (by - ay) * (x - ax), (cx - bx) * (y - by) - (cy - by) * (x - bx), (ax - cx) * (y - cy) - (ay - cy) * (x - cx)];
    return d.every(v => v >= 0) || d.every(v => v <= 0);
}

/** Whether world point x, y lies on a carriageway. */
export function onWay(ways: Ways, x: number, y: number): boolean {
    let on = false;
    const unit = metresPerUnit(y);
    near(ways, x, y, x, y, 0, s => {
        const { xy: [ax, ay, bx, by], h, quad } = ways.segments[s];
        if (quad) on ||= inTriangle(x, y, quad.slice(0, 6)) || inTriangle(x, y, [...quad.slice(4, 8), ...quad.slice(0, 2)]);
        else {
            const t = along(ax, ay, bx, by, x, y);
            on ||= Math.hypot(ax + t * (bx - ax) - x, ay + t * (by - ay) - y) * unit < h - INSIDE_M;
        }
    });
    return on;
}
