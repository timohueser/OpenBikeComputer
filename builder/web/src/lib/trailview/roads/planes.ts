// The heights of a junction. A side arm's first metres lie on the through road's banked plane, so its mouth meets
// that edge level; the bake's heights hold the through road's middle only. The stamped ground holds a side arm at
// its own heights, and it never sinks under them: where the through edge lies lower, that edge rises instead.
import { footOn, outward, type Way } from './arms';
import { KERB_RISE_M } from './section';
import { stationAt, type Station } from './stations';
import type { Mouth } from './junctions';
import type { PathEnd } from './path-ends';

/** A side arm's edge leaves the through road's plane over this many metres past its mouth, by at most SHIFT_M. */
const CONFORM_M = 10, SHIFT_M = 0.5;
/** A through edge rises at most RAISE_M about its other edge, fully within FLAT_M of the junction or along the mouth, and back over FLATTEN_M more. */
const RAISE_M = 0.3, FLAT_M = 4, FLATTEN_M = 10;
/** A path's tread ends this far under a road's shoulder. */
const UNDER_SHOULDER_M = 0.02;

const smooth = (t: number) => t * t * (3 - 2 * t);
const clamp = (t: number) => Math.max(0, Math.min(1, t));

/** The height of a way's surface at a point near it, without its lift over the core: its banked plane, held level past its edges. */
export function planeAt(way: Way, x: number, y: number): { z: number; left: number; away: number } {
    const { s, t, u, left, away } = footOn(way, x, y), mix = (a: number, b: number) => a + (b - a) * u, h = mix(s.h, t.h);
    return { z: mix(s.z, t.z) + mix(s.bank, t.bank) * Math.max(-h, Math.min(h, left)) + way.run.lift, left, away };
}

/** The through road's plane under a point: of the nearer of its ways. */
function throughPlane(through: Way[], x: number, y: number): { z: number } {
    return through.map(t => planeAt(t, x, y)).reduce((a, b) => (b.away < a.away ? b : a));
}

/** A station's two edges: their own height over the core and the through road's plane there, without the way's lift. */
function edgesOf(s: Station, through: Way[], lift: number): { own: number; plane: number }[] {
    return [1, -1].map(side => ({ own: s.z + s.bank * side * s.h, plane: throughPlane(through, s.x + s.ax * side * s.h, s.y + s.ay * side * s.h).z - lift }));
}

/** How far each through road's edges rise for the mouths whose side arms lie higher: per station, left and right. */
function raises(mouths: Mouth[]): Map<Station, [number, number]> {
    const out = new Map<Station, [number, number]>();
    for (const { way, far, through, reach } of mouths) {
        const at = way.run.stations[stationAt(way.run.stations, far)], rise = Math.min(RAISE_M, Math.max(...edgesOf(at, through, way.run.lift).map(({ own, plane }) => own - plane)));
        if (rise <= 0) continue;
        for (const t of through.filter((one, i) => through.findIndex(({ run }) => run === one.run) === i)) {
            const side = planeAt(t, at.x, at.y).left > 0 ? 0 : 1, { stations } = t.run, flat = Math.max(FLAT_M, reach);
            for (let j = stationAt(stations, t.along - flat - FLATTEN_M); j < stations.length && stations[j].along <= t.along + flat + FLATTEN_M; j++) {
                const w = 1 - smooth(clamp((Math.abs(stations[j].along - t.along) - flat) / FLATTEN_M)), r = out.get(stations[j]) ?? out.set(stations[j], [0, 0]).get(stations[j])!;
                r[side] = Math.max(r[side], rise * w);
            }
        }
    }
    return out;
}

/** Moves a side arm's edges onto the through road's plane up to its mouth's end, and back to their own heights past it. */
function conform({ way, far, through }: Mouth): void {
    const { stations, lift } = way.run, metres = Math.abs(far - way.along), shifts = edgesOf(stations[stationAt(stations, far)], through, lift).map(({ own, plane }) => Math.max(0, Math.min(SHIFT_M, plane - own)));
    for (const s of outward(way, metres + CONFORM_M)) {
        const d = Math.abs(s.along - way.along);
        if (d > metres + CONFORM_M) break;
        const fade = 1 - smooth(Math.max(0, d - metres) / CONFORM_M), [left, right] = edgesOf(s, through, lift).map(({ own, plane }, side) => (d <= metres ? Math.max(plane, own) : own + shifts[side] * fade));
        s.z = (left + right) / 2;
        s.bank = (left - right) / (2 * s.h);
    }
}

/** The heights that a tile's junctions change. */
export function settleHeights(mouths: Mouth[], ends: PathEnd[]): void {
    for (const [s, [left, right]] of raises(mouths)) {
        s.z += (left + right) / 2;
        s.bank += (left - right) / (2 * s.h);
    }
    for (const mouth of mouths) conform(mouth);
    for (const { way, at, road, walk } of ends) {
        const s = way.run.stations[stationAt(way.run.stations, at)];
        s.z = planeAt(road, s.x, s.y).z + (walk ? KERB_RISE_M : -UNDER_SHOULDER_M) - way.run.lift;
    }
}
