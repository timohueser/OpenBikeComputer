// Whether a road's cut has room for its slope, per station and side: the next way's core along the side, whose plane
// the stamp holds over every cone (stamp-core.wgsl), and whether the cones of the two ways meet between the cores.
// Where they do not, the stamp ramps from the one core to the other, and a retaining wall holds that ground (standing.ts).
import { metresPerUnit, PROFILES, SHOULDER_M, type Line } from '../terrain/bins';
import { along, near } from '../terrain/town';
import type { GroundAt } from './ground';
import { sideOf } from './section';
import { widen } from './skirt';
import { between, stationsOf, type Station } from './stations';
import type { Ways } from './ways';

/** The back of a wall's backfill, metres past the core's edge (details.ts): a wall holds the ground there. */
export const WALL_OUT_M = 2.2;
/** The search's step out along a side, and its bisections of the step that enters a core. */
const STEP_M = 0.25, HALVINGS = 3;
/** The most that a core reaches past its carriageway: a sidewalk or a shoulder. */
const PAST_M = Math.max(...Object.values(PROFILES).map(({ walk }) => walk), ...Object.values(SHOULDER_M));

/**
 * One side of a station: `open` where its cut has room, `held`, the stamped ground at the back of a wall's backfill over
 * the core's edge, at least 0, `free`, the metres past the core's edge to the next core, Infinity where none lies, and
 * `meets` where another way's core holds the core's edge: a way meets the road on this side, as at a junction's mouth.
 */
export interface Room { open: boolean; held: number; free: number; meets: boolean }

/** The metres from a line's middle to its core's outer edge on a side, where the stamp's cone starts and a wall stands. */
export const coreEdgeOf = (line: Line, s: Station, side: 1 | -1) => sideOf(line, side, s.v, s.h).edge;

/** A segment of another way, with its core's metres on its left and its right. */
interface Core { segment: Ways['segments'][number]; left: number; right: number }

/** Whether world point x, y lies in a core. */
function inside({ segment: { xy: [ax, ay, bx, by] }, left, right }: Core, x: number, y: number): boolean {
    const t = along(ax, ay, bx, by, x, y), d = Math.hypot(ax + t * (bx - ax) - x, ay + t * (by - ay) - y) * metresPerUnit(y);
    // World y runs south: right of the segment, the cross product is positive.
    return d < ((bx - ax) * (y - ay) - (by - ay) * (x - ax) > 0 ? right : left);
}

const drawn = new WeakMap<Line, Station[]>();

/** The height of segment v's plane at world point x, y, as the way's corridor draws it: between its two nearest stations, with their bank. */
function planeAt(line: Line, v: number, x: number, y: number): number {
    // A station's height and bank do not hang on its half-width.
    const stations = drawn.get(line) ?? drawn.set(line, stationsOf(line, () => line.h)).get(line)!;
    let best = Infinity, z = NaN;
    for (let k = 0; k + 1 < stations.length; k++) {
        const s = stations[k], t = stations[k + 1];
        if (t.v < v || s.v > v) continue;
        const u = along(s.x, s.y, t.x, t.y, x, y), at = between(s, t, u), d = Math.hypot(at.x - x, at.y - y);
        if (d < best) [best, z] = [d, at.z + at.bank * ((x - at.x) * at.ax + (y - at.y) * at.ay) / (at.ax ** 2 + at.ay ** 2)];
    }
    return z;
}

/**
 * The first core that a side enters past open ground, out to `reach` metres from the core's edge at x0, y0 along ox, oy
 * (world units per metre): its metres, its plane's height there and its line; and whether a way meets `road` there. A
 * core that holds the station's middle or the edge is the way itself here, or a way that it meets. A way that runs more
 * across the road than along it crosses or joins the road, as a bridge's approach or a side road does: an abutment or a
 * mouth holds that ground.
 */
function nextCore(road: Line, ways: Ways, s: Station, x0: number, y0: number, ox: number, oy: number, reach: number): { next?: { metres: number; z: number; line: Line }; meets: boolean } {
    const cores: Core[] = [], at = (metres: number) => [x0 + ox * metres, y0 + oy * metres] as const;
    let meets = false;
    near(ways, x0, y0, x0 + ox * reach, y0 + oy * reach, PAST_M / metresPerUnit(y0), k => {
        const segment = ways.segments[k], { line, v, h, xy: [ax, ay, bx, by] } = segment, core = { segment, left: sideOf(line, 1, v, h).edge, right: sideOf(line, -1, v, h).edge };
        const beside = Math.abs((bx - ax) * oy - (by - ay) * ox) >= Math.abs((bx - ax) * ox + (by - ay) * oy), edge = inside(core, x0, y0);
        meets ||= edge && line !== road;
        if (beside && !edge && !inside(core, s.x, s.y)) cores.push(core);
    });
    for (let metres = STEP_M; cores.length && metres < reach + STEP_M; metres += STEP_M) {
        const core = cores.find(c => inside(c, ...at(Math.min(metres, reach))));
        if (!core) continue;
        // The step's crossing into that core: its metres the last point outside, its height the first inside.
        let [lo, hi] = [metres - STEP_M, Math.min(metres, reach)];
        for (let i = 0; i < HALVINGS; i++) {
            const mid = (lo + hi) / 2;
            [lo, hi] = inside(core, ...at(mid)) ? [lo, mid] : [mid, hi];
        }
        const { line, v } = core.segment;
        return { next: { metres: lo, z: planeAt(line, v, ...at(hi)), line }, meets };
    }
    return { meets };
}

/**
 * The room of station s's cut on `side`. The stamp holds the next core at its plane, `dz` over this core's edge, `g`
 * metres out. This way's cut, tol + widen(x, cut), and the other's fill, tol + widen(g - x, fill), meet between the
 * cores where their sum reaches dz; widen is convex, so they meet if and only if one of them alone reaches dz across g.
 * Else the stamp ramps between the cones, steeper than either: no room. With no core within the cone's reach, the cut
 * meets the natural ground, as the cone grows to the vertical there.
 */
export function roomOf(line: Line, s: Station, side: 1 | -1, ground: GroundAt, ways: Ways): Room {
    const { tol, reach, cut, fill } = Object.values(PROFILES)[line.profile], e = coreEdgeOf(line, s, side), edge = s.z + side * s.bank * e;
    const ox = side * s.ax, oy = side * s.ay, x0 = s.x + ox * e, y0 = s.y + oy * e, { next, meets } = nextCore(line, ways, s, x0, y0, ox, oy, reach);
    const other = next && Object.values(PROFILES)[next.line.profile], dz = next ? next.z - edge : 0, g = next?.metres ?? Infinity;
    const open = !other || !(dz - tol - other.tol > Math.max(widen(g, reach, cut), widen(g, other.reach, other.fill)));
    if (next && g <= WALL_OUT_M) return { open, held: Number.isNaN(dz) ? 0 : Math.max(0, dz), free: g, meets };
    // The stamp at the back of the backfill, as stamp-core.wgsl bounds its cones: the higher fill and the lower cut, each
    // with the metres from the way that holds it; where they cross, the ramp between them by those metres.
    const x = WALL_OUT_M, y = g - x;
    let [lo, loX, hi, hiX] = [-tol - widen(x, reach, fill), x, tol + widen(x, reach, cut), x];
    if (other) {
        const [l, u] = [dz - other.tol - widen(y, other.reach, other.fill), dz + other.tol + widen(y, other.reach, other.cut)];
        if (l > lo) [lo, loX] = [l, y];
        if (u < hi) [hi, hiX] = [u, y];
    }
    const held = lo > hi ? lo + (hi - lo) * loX / (loX + hiX) : Math.min(hi, Math.max(lo, ground(x0 + ox * x, y0 + oy * x) - edge));
    return { open, held: Number.isNaN(held) ? 0 : Math.max(0, held), free: g, meets };
}
