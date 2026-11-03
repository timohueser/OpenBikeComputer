// The daylight line of a corridor's skirt: per station and side, the metres past the core's edge where the
// way's cut or fill meets the natural ground, by the cone rule of the stamp (stamp-core.wgsl) on the stored ground,
// or before it reaches a carriageway, whose ground that way's corridor draws, as the stamp ranks a core over a cone.
import { PATH, PROFILES, type Line } from '../terrain/bins';
import type { GroundAt } from './ground';
import { sideOf } from './section';
import type { Station } from './stations';
import { onWay, type Ways } from './ways';

/** The metres past the core's edge of a station's daylight line, `most` 0 on its left and 1 on its right. */
export type Daylight = (j: number, most: number) => number;

/** The search's step out from the core's edge, and its bisections of the last step that binds. */
const STEP_M = 1, PATH_STEP_M = 0.5, HALVINGS = 3;
/** A path's bench: BENCH_TEXELS of stamp-core.wgsl at the finest level's texel of 0.4 m. */
const BENCH_M = 0.5;

/** A height that grows at `slope` and goes to infinity at `reach`, as `widen` of stamp-core.wgsl. */
export const widen = (x: number, reach: number, slope: number) => (x >= reach ? Infinity : slope * x * reach / (reach - x));

/** Whether the cone of a line binds the ground `g` at `x` metres past its core's edge, on a plane at height `z` there. */
function binds(line: Line, x: number, z: number, g: number): boolean {
    const { tol, reach, cut, fill, tilt } = Object.values(PROFILES)[line.profile];
    if (line.kind === PATH) return g < z - tol - tilt * line.h - widen(x, reach, fill) || g > z + tol + widen(Math.max(0, x - BENCH_M), reach - BENCH_M, cut);
    return g < z - tol - widen(x, reach, fill) || g > z + tol + widen(x, reach, cut);
}

/**
 * The daylight of a line's stations on the ground, worked out once per station and side as the sweep reads it; 0 where
 * the ground is unknown. With the tile's `ways`, it ends a step before a carriageway, another leg of its own too.
 */
export function daylightOf(line: Line, stations: Station[], ground: GroundAt, ways?: Ways): Daylight {
    const known = new Float64Array(2 * stations.length).fill(NaN), { reach } = Object.values(PROFILES)[line.profile], step = line.kind === PATH ? PATH_STEP_M : STEP_M;
    return (j, most) => {
        if (!Number.isNaN(known[2 * j + most])) return known[2 * j + most];
        const s = stations[j], side = most === 0 ? 1 : -1, edge = sideOf(line, side, s.v, s.h).edge;
        const bound = (x: number) => {
            const a = side * (edge + x), g = ground(s.x + s.ax * a, s.y + s.ay * a);
            return !Number.isNaN(g) && binds(line, x, s.z + s.bank * a, g);
        };
        // The last step that binds, then the crossing past it; at most the last step before a carriageway, so the
        // skirt's last row, under the mesh, ends on that carriageway.
        let last = -1, end = reach;
        for (let x = 0; x < end; x += step) {
            const a = side * (edge + x);
            if (ways && onWay(ways, s.x + s.ax * a, s.y + s.ay * a)) end = x - step;
            else if (bound(x)) last = x;
        }
        let metres = 0;
        if (last >= 0) {
            let [lo, hi] = [last, Math.min(last + step, reach)];
            for (let i = 0; i < HALVINGS; i++) {
                const mid = (lo + hi) / 2;
                [lo, hi] = bound(mid) ? [mid, hi] : [lo, mid];
            }
            metres = Math.max(0, Math.min(hi, end));
        }
        return (known[2 * j + most] = metres);
    };
}
