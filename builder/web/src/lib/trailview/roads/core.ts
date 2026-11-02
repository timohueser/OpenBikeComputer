// The core of a way across: from its middle out to the outer edge of each side, past the carriageway its
// kerb and sidewalk, its shoulder or a track's spill, which the stamp holds flat. Nothing may stand in it.
import type { Line } from '../terrain/bins';
import { sideOf } from './section';
import type { Station } from './stations';

/** The metres from a way's middle out to the outer edge of its side `side`, 1 left and -1 right, at station s. */
export const coreEdge = (line: Line, s: Station, side: 1 | -1) => sideOf(line, side, s.v, s.h).edge;

/**
 * Whether world point x, y lies in the core of a way: within coreEdge across at the way's nearest station,
 * on the point's side, and not past its first or last station. The stations are those of the corridor,
 * `stationsOf(line, v => halfAt(line, v, partners))`.
 */
export function inCore(line: Line, stations: Station[], x: number, y: number): boolean {
    let j = -1, least = Infinity;
    stations.forEach((s, k) => {
        const d = (s.x - x) ** 2 + (s.y - y) ** 2;
        if (d < least) [j, least] = [k, d];
    });
    if (j < 0) return false;
    // ax, ay point left in world units per metre, and -ay, ax ahead: a dot product with them over their square is metres.
    const s = stations[j], square = s.ax * s.ax + s.ay * s.ay, across = ((x - s.x) * s.ax + (y - s.y) * s.ay) / square, along = ((y - s.y) * s.ax - (x - s.x) * s.ay) / square;
    if ((j === 0 && along < 0) || (j === stations.length - 1 && along > 0)) return false;
    return Math.abs(across) <= coreEdge(line, s, across >= 0 ? 1 : -1);
}
