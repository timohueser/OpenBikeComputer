// The daylight line of a corridor's skirt: per station and side, the metres past the core's edge where the
// way's cut or fill meets the natural ground, by the cone rule of the stamp (stamp-core.wgsl) on the stored ground,
// or where it reaches another way's core, whose ground that way's corridor draws, as the stamp ranks a core over a cone.
import { metresPerUnit, PATH, PROFILES, type Line } from '../terrain/bins';
import { along, gridOf, near, type Grid } from '../terrain/town';
import { isDeck } from './decks';
import type { GroundAt } from './ground';
import { corridorLine, sideOf } from './section';
import type { Station } from './stations';

/** The metres past the core's edge of a station's daylight line, `most` 0 on its left and 1 on its right. */
export type Daylight = (j: number, most: number) => number;

/** The search's step out from the core's edge, and its bisections of the last step that binds. */
const STEP_M = 1, PATH_STEP_M = 0.5, HALVINGS = 3;
/** A path's bench: BENCH_TEXELS of stamp-core.wgsl at the finest level's texel of 0.4 m. */
const BENCH_M = 0.5;

/** A height that grows at `slope` and goes to infinity at `reach`, as `widen` of stamp-core.wgsl. */
const widen = (x: number, reach: number, slope: number) => (x >= reach ? Infinity : slope * x * reach / (reach - x));

/** Whether the cone of a line binds the ground `g` at `x` metres past its core's edge, on a plane at height `z` there. */
function binds(line: Line, x: number, z: number, g: number): boolean {
    const { tol, reach, cut, fill, tilt } = Object.values(PROFILES)[line.profile];
    if (line.kind === PATH) return g < z - tol - tilt * line.h - widen(x, reach, fill) || g > z + tol + widen(Math.max(0, x - BENCH_M), reach - BENCH_M, cut);
    return g < z - tol - widen(x, reach, fill) || g > z + tol + widen(x, reach, cut);
}

/** The cores of a tile's ways with a corridor on the ground, per segment: its line, its ends and its core's half-width. */
export interface CoreIndex extends Grid { segments: { line: Line; xy: number[]; half: number }[] }

export function coreIndex(lines: Line[]): CoreIndex {
    const segments: CoreIndex['segments'] = [];
    for (const line of lines.filter(way => corridorLine(way) && !isDeck(way))) {
        for (let v = 0; v + 1 < line.z.length; v++) segments.push({ line, xy: line.xy.slice(2 * v, 2 * v + 4), half: Math.max(sideOf(line, 1, v, line.h).edge, sideOf(line, -1, v, line.h).edge) });
    }
    const boxes = segments.map(({ xy: [ax, ay, bx, by], half }) => {
        const r = half / metresPerUnit(ay);
        return [Math.min(ax, bx) - r, Math.min(ay, by) - r, Math.max(ax, bx) + r, Math.max(ay, by) + r];
    });
    return { ...gridOf(boxes), segments };
}

/** Whether world point x, y lies in the core of a way other than `line`. */
function inCore(cores: CoreIndex, line: Line, x: number, y: number): boolean {
    let inside = false;
    const unit = metresPerUnit(y);
    near(cores, x, y, x, y, 0, s => {
        const { line: other, xy: [ax, ay, bx, by], half } = cores.segments[s], t = along(ax, ay, bx, by, x, y);
        inside ||= other !== line && Math.hypot(ax + t * (bx - ax) - x, ay + t * (by - ay) - y) * unit < half;
    });
    return inside;
}

/**
 * The daylight of a line's stations on the ground, worked out once per station and side as the sweep reads it; 0 where
 * the ground is unknown. With the tile's `cores`, it ends where it reaches another way's core.
 */
export function daylightOf(line: Line, stations: Station[], ground: GroundAt, cores?: CoreIndex): Daylight {
    const known = new Float64Array(2 * stations.length).fill(NaN), { reach } = Object.values(PROFILES)[line.profile], step = line.kind === PATH ? PATH_STEP_M : STEP_M;
    return (j, most) => {
        if (!Number.isNaN(known[2 * j + most])) return known[2 * j + most];
        const s = stations[j], side = most === 0 ? 1 : -1, edge = sideOf(line, side, s.v, s.h).edge;
        const bound = (x: number) => {
            const a = side * (edge + x), g = ground(s.x + s.ax * a, s.y + s.ay * a);
            return !Number.isNaN(g) && binds(line, x, s.z + s.bank * a, g);
        };
        // The last step that binds, then the crossing past it; at most the last step before another way's core, so the
        // skirt's last row, under the mesh, ends in that core.
        let last = -1, end = reach;
        for (let x = 0; x < end; x += step) {
            const a = side * (edge + x);
            if (cores && inCore(cores, line, s.x + s.ax * a, s.y + s.ay * a)) end = x - step;
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
