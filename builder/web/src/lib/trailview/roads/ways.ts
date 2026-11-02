// The carriageways of a tile, in a grid: a band's row over another way sinks under it, so that way draws over it.
import { metresPerUnit, type Line } from '../terrain/bins';
import { along, gridOf, near, type Grid } from '../terrain/town';
import { isDeck } from './decks';
import { corridorLine } from './section';

/** A point this far inside a carriageway's edge lies on it. */
const INSIDE_M = 0.05;

export interface Ways extends Grid { segments: { xy: number[]; h: number }[] }

export function waysOf(lines: Line[]): Ways {
    const segments: Ways['segments'] = [];
    // A deck stands over the bands under it.
    for (const line of lines.filter(way => corridorLine(way) && !isDeck(way))) {
        for (let v = 0; v + 1 < line.z.length; v++) segments.push({ xy: line.xy.slice(2 * v, 2 * v + 4), h: line.h + (line.plan?.widen[v] ?? 0) });
    }
    const boxes = segments.map(({ xy: [ax, ay, bx, by], h }) => {
        const r = h / metresPerUnit(ay);
        return [Math.min(ax, bx) - r, Math.min(ay, by) - r, Math.max(ax, bx) + r, Math.max(ay, by) + r];
    });
    return { ...gridOf(boxes), segments };
}

/** Whether world point x, y lies on a carriageway. */
export function onWay(ways: Ways, x: number, y: number): boolean {
    let on = false;
    const unit = metresPerUnit(y);
    near(ways, x, y, x, y, 0, s => {
        const { xy: [ax, ay, bx, by], h } = ways.segments[s], t = along(ax, ay, bx, by, x, y);
        on ||= Math.hypot(ax + t * (bx - ax) - x, ay + t * (by - ay) - y) * unit < h - INSIDE_M;
    });
    return on;
}
