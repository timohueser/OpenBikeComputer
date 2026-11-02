// The heights of a junction. A side arm's first metres lie on the through road's banked plane, so its mouth meets
// that edge level; the bake's heights hold the through road's middle only. The stamped ground holds a side arm at
// its own heights, and it never sinks under them: where the through edge lies lower, that edge rises instead, and
// the mouth's faces close what RAISE_M leaves.
import { footOn, outward, sideAt, type Way } from './arms';
import { CORE_M, isDeck, PARAPET_M } from './decks';
import { onward, type Joints } from './joints';
import { KERB_RISE_M } from './section';
import { stationOn, type Station } from './stations';
import type { Mouth } from './junctions';
import type { PathEnd } from './path-ends';

/** A side arm's edge leaves the through road's plane over this many metres past its mouth. */
const CONFORM_M = 10;
/** A through edge rises at most RAISE_M about its other edge, fully within FLAT_M of the junction or along the mouth, and back over FLATTEN_M more. */
const RAISE_M = 0.3, FLAT_M = 4, FLATTEN_M = 10;
/** A path's tread ends this far under a road's shoulder. */
const UNDER_SHOULDER_M = 0.02;
/** A way's end meets the road's plane and returns to its own heights over this many metres at least, at this grade at most. */
const END_LEAST_M = 2, END_GRADE = 0.3;

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

/** How far each through road's edges rise for the mouths whose side arms lie higher, on across its joints: per station, left and right. */
function raises(mouths: Mouth[], joints: Joints): Map<Station, [number, number]> {
    const out = new Map<Station, [number, number]>();
    for (const { way, far, through, reach } of mouths) {
        const at = stationOn(way.run.stations, far), rise = Math.min(RAISE_M, Math.max(...edgesOf(at, through, way.run.lift).map(({ own, plane }) => own - plane))), flat = Math.max(FLAT_M, reach);
        if (rise <= 0) continue;
        for (const t of through) {
            const side = planeAt(t, at.x, at.y).left > 0 ? 0 : 1;
            for (const { s, metres, flip } of onward(joints, t.run, t.along, t.step, flat + FLATTEN_M)) {
                const edges = out.get(s) ?? out.set(s, [0, 0]).get(s)!, edge = flip > 0 ? side : 1 - side;
                edges[edge] = Math.max(edges[edge], rise * (1 - smooth(clamp((metres - flat) / FLATTEN_M))));
            }
        }
    }
    return out;
}

/** The metres of a side arm from its junction that its plane holds: to its mouth's end, and on while its core lies on the footprint of a deck that it leaves. */
function heldTo({ way, far, through }: Mouth): number {
    const decks = through.filter(t => isDeck(t.line)), core = way.line.h + CORE_M;
    let reach = Math.abs(far - way.along);
    const on = (x: number, y: number) => decks.some(t => {
        const { left, away, behind } = footOn(t, x, y);
        return behind === 0 && away <= sideAt(t, left > 0 ? 1 : -1).edge + PARAPET_M;
    });
    if (decks.length) for (const s of outward(way, reach + CONFORM_M)) if (Math.abs(s.along - way.along) > reach && [-core, 0, core].some(a => on(s.x + s.ax * a, s.y + s.ay * a))) reach = Math.abs(s.along - way.along);
    return reach;
}

/** Moves a side arm's edges onto the through road's plane as far as `heldTo`, and back to their own heights past it, on across its joints. */
function conform(mouth: Mouth, joints: Joints): void {
    const { way, through } = mouth, { stations, lift } = way.run, reach = heldTo(mouth), shifts = edgesOf(stationOn(stations, way.along + way.step * reach), through, lift).map(({ own, plane }) => Math.max(0, plane - own));
    // A through way can be this line, so every height is read before any is written.
    const reached = [...onward(joints, way.run, way.along, way.step, reach + CONFORM_M)];
    // A deck's structure stands at the bake's heights: the fade ends on the last station short of it.
    const deck = reached.findIndex(({ run }) => run !== way.run && isDeck(run.line)), fadeM = deck > 0 ? reached[deck - 1].metres - reach : CONFORM_M;
    const moved = reached.slice(0, deck < 0 ? undefined : deck).map(({ s, metres, flip }) => {
        const fade = 1 - smooth(clamp((metres - reach) / fadeM)), edge = (side: number, own: number) => own + shifts[flip > 0 ? side : 1 - side] * fade;
        const [left, right] = metres <= reach ? edgesOf(s, through, lift).map(({ own, plane }) => Math.max(plane, own)) : [edge(0, s.z + s.bank * s.h), edge(1, s.z - s.bank * s.h)];
        return { s, left, right };
    });
    for (const { s, left, right } of moved) {
        s.z = (left + right) / 2;
        s.bank = (left - right) / (2 * s.h);
    }
}

/** The heights that a tile's junctions change. */
export function settleHeights(mouths: Mouth[], ends: PathEnd[], joints: Joints): void {
    for (const [s, [left, right]] of raises(mouths, joints)) {
        s.z += (left + right) / 2;
        s.bank += (left - right) / (2 * s.h);
    }
    for (const mouth of mouths) conform(mouth, joints);
    for (const { way, at, roads, walk } of ends) {
        const { x, y, z } = stationOn(way.run.stations, at), change = throughPlane(roads, x, y).z + (walk ? KERB_RISE_M : -UNDER_SHOULDER_M) - way.run.lift - z, over = Math.max(END_LEAST_M, Math.abs(change) / END_GRADE);
        for (const { s, metres } of onward(joints, way.run, at, way.step, over)) s.z += change * (1 - smooth(clamp(metres / over)));
    }
}
