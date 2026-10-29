// PROTOTYPE road system (a study; never merges). The stations along a line: points on its curve at most
// STEP_M apart, each with its height, its unit vector across to the left in world units per metre, its
// metres from the start, its segment, its bank and, on a track, its sleepers from the start.
import { metresPerUnit, type Line } from '../terrain/bins';

/** The longest step between two stations: the band anchors to the drawn mesh at each. */
export const STEP_M = 2;

export interface Station {
    x: number;
    y: number;
    z: number;
    /** Across to the left, world units per metre. */
    ax: number;
    ay: number;
    along: number;
    /** The segment of the line that the station lies on, for the per-segment data (sidewalks). */
    v: number;
    /** The surface's slope across, rise per metre to the left: crossfall in bends. */
    bank: number;
    sleepers?: number;
    /** The station's index along its line; none for a made-up station. */
    j?: number;
}

/** The stations of a line, in order. */
export function stationsOf(line: Line, step = STEP_M): Station[] {
    const n = line.z.length, pts: Omit<Station, 'ax' | 'ay' | 'bank'>[] = [];
    let along = 0;
    for (let v = 0; v + 1 < n; v++) {
        const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), metres = Math.hypot(bx - ax, by - ay) * metresPerUnit(ay);
        const steps = Math.max(1, Math.ceil(metres / step));
        for (let k = v === 0 ? 0 : 1; k <= steps; k++) {
            const u = k / steps, sl = line.sleepers;
            pts.push({ x: ax + (bx - ax) * u, y: ay + (by - ay) * u, z: line.z[v] + (line.z[v + 1] - line.z[v]) * u, along: along + metres * u, v: Math.min(v, n - 2), sleepers: sl ? sl[v] + (sl[v + 1] - sl[v]) * u : undefined });
        }
        along += metres;
    }
    return pts.map((p, j) => {
        const a = pts[Math.max(0, j - 1)], c = pts[Math.min(pts.length - 1, j + 1)];
        const dx = c.x - a.x, dy = c.y - a.y, l = Math.hypot(dx, dy) || 1, m = metresPerUnit(p.y);
        return { ...p, ax: dy / l / m, ay: -dx / l / m, bank: 0, j };
    });
}
