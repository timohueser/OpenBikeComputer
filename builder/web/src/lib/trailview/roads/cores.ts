// The cores of a tile's ways as the corridors draw them: from a way's middle out to the outer edge of each
// side, past the carriageway its kerb and sidewalk, its shoulder or a track's spill. Models at points keep
// out of them, and the census counts what stands in them and what no face draws there.
import { junctions, metresPerUnit, type Line } from '../terrain/bins';
import { halfAt, partnersOf } from '../terrain/corners';
import { corridorLine, sideOf } from './section';
import { between, stationsOf, type Station } from './stations';

/**
 * The ways whose cores the models at points keep out of: the tile's roads, tracks, paths, railways and
 * bridge decks with a corridor, the partners that set a road's half-width at its ends, and the points that two of them share.
 */
export interface Cores { ways: Line[]; partners: Map<Line, [Line?, Line?]>; shared: { x: number; y: number; h: number }[] }

const coresByTile = new WeakMap<Line[], Cores>(), stationsByLine = new WeakMap<Line, Station[]>();

/** The cores of a tile's lines, made once per tile. */
export function coresOf(lines: Line[]): Cores {
    let cores = coresByTile.get(lines);
    if (!cores) {
        const ways = lines.filter(line => corridorLine(line) && line.z.length > 1);
        coresByTile.set(lines, (cores = { ways, partners: partnersOf(lines), shared: [...junctions(ways).values()] }));
    }
    return cores;
}

/** The stations of a way, as its corridor sweeps them. */
export function stationsIn(cores: Cores, line: Line): Station[] {
    return stationsByLine.get(line) ?? stationsByLine.set(line, stationsOf(line, v => halfAt(line, v, cores.partners))).get(line)!;
}

/** A way's core is at most this many metres past its half-width: a sidewalk or a shoulder. */
export const PAST_HALF_M = 6;

/** Of `ways`, those whose core may reach within `metres` of world point x, y. */
export function waysNear(ways: Line[], x: number, y: number, metres: number): Line[] {
    const unit = metresPerUnit(y);
    return ways.filter(({ xy, h }) => {
        const r = (h + PAST_HALF_M + metres) / unit;
        for (let i = 0; i + 3 < xy.length; i += 2) {
            if (Math.min(xy[i], xy[i + 2]) - r < x && Math.max(xy[i], xy[i + 2]) + r > x && Math.min(xy[i + 1], xy[i + 3]) - r < y && Math.max(xy[i + 1], xy[i + 3]) + r > y) return true;
        }
        return false;
    });
}

/** The metres from a way's middle to the outer edge of its core on `side`, 1 left and -1 right, at `foot` on the step from station s. */
export const coreEdge = (line: Line, s: Station, foot: Station, side: 1 | -1) => sideOf(line, side, s.v, foot.h).edge;

/**
 * The core of `ways` that holds world point x, y most deeply, widened by `past` metres, as the corridors
 * draw it: the point's foot on the way's stations and its side, 1 left and -1 right; undefined outside every core.
 */
export function coreAt(cores: Cores, ways: Line[], x: number, y: number, past = 0): { foot: Station; side: 1 | -1 } | undefined {
    const unit = metresPerUnit(y);
    let deepest = 0, found: ReturnType<typeof coreAt>;
    for (const line of ways) {
        const st = stationsIn(cores, line);
        for (let k = 0; k + 1 < st.length; k++) {
            const s = st[k], dx = st[k + 1].x - s.x, dy = st[k + 1].y - s.y, l = dx * dx + dy * dy;
            const raw = l ? ((x - s.x) * dx + (y - s.y) * dy) / l : 0, u = Math.min(1, Math.max(0, raw));
            // Past a way's end its core stops.
            if ((k === 0 && raw < 0) || (k + 2 === st.length && raw > 1)) continue;
            const d = Math.hypot(s.x + u * dx - x, s.y + u * dy - y) * unit;
            if (d - s.h > PAST_HALF_M + past) continue;
            // A core's width changes from a station to the next, where a sidewalk starts: each step holds its own.
            const foot = between(s, st[k + 1], u), side = (x - foot.x) * foot.ax + (y - foot.y) * foot.ay >= 0 ? 1 : -1;
            const depth = coreEdge(line, s, foot, side) + past - d;
            if (depth > deepest) [deepest, found] = [depth, { foot, side }];
        }
    }
    return found;
}
