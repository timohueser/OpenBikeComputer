// Cable cars and ski lifts: a station at each end of the line, a pylon at each vertex between (the bake
// adds the pylons that OSM lacks), two cables that sag from anchor to anchor, and carriers on them.
import { carrier, pylon, station, TRACK_HALF_M, type Rig } from '../models/aerialways';
import type { Model } from '../models/flat';
import { metresBetween } from '../structures/anchored';
import { rgb } from '../structures/mesh';
import type { Kind } from './kind';

/** As RIGS of tools/trailview/lifts.py: the cable's height over the ground at a pylon and at a station, and its sag as a share of the span; and the spacing of the carriers, 0 for a cable car's two cabins. */
const RIGS: Record<Rig, { pylon: number; station: number; sag: number; spacing: number }> = {
    cable_car: { pylon: 30, station: 8.4, sag: 0.04, spacing: 0 },
    gondola: { pylon: 14, station: 6, sag: 0.02, spacing: 70 },
    chair_lift: { pylon: 10, station: 4.2, sag: 0.02, spacing: 30 },
    drag_lift: { pylon: 7, station: 4.5, sag: 0.01, spacing: 15 },
};
const CLASSES: Record<string, Rig> = {
    cable_car: 'cable_car', gondola: 'gondola', mixed_lift: 'gondola', chair_lift: 'chair_lift', drag_lift: 'drag_lift', 't-bar': 'drag_lift', 'j-bar': 'drag_lift', platter: 'drag_lift',
};
const CABLE = rgb('#2b2b2b');
/** A cable is straight to the eye in pieces this long. */
const PIECE_M = 20;

const models = new Map<string, Model>();
const model = (name: string, make: () => Model) => models.get(name) ?? models.set(name, make()).get(name)!;

export default {
    build({ tags, parts }, out) {
        const rig = CLASSES[tags.aerialway], { anchored } = out.far;
        if (!rig) return;
        const { pylon: high, station: low, sag, spacing } = RIGS[rig], T = TRACK_HALF_M[rig];
        for (const { xy, ends } of parts) {
            const n = xy.length / 2, at = (v: number): [number, number] => [xy[2 * v], xy[2 * v + 1]];
            // One livery per lift: by its heading, which each tile's piece of a straight lift shares.
            const seed = Math.round(Math.atan2(xy[2 * n - 1] - xy[1], xy[2 * n - 2] - xy[0]) * 50);
            const end = (v: number) => (v === 0 && ends[0]) || (v === n - 1 && ends[1]);
            const yaw = (from: number, to: number) => Math.atan2(xy[2 * from + 1] - xy[2 * to + 1], xy[2 * to] - xy[2 * from]);
            for (let v = 0; v < n; v++) {
                if (!out.owns(...at(v))) continue;
                if (end(v)) anchored.model(model(`station ${rig} ${seed}`, () => station(rig, seed)), anchored.point(...at(v), v ? yaw(v, v - 1) : yaw(v, v + 1)));
                // A pylon stays in the anchored mesh: its foot is the end of its cables' spans.
                else if (v > 0 && v < n - 1) anchored.model(model(`pylon ${rig} ${seed}`, () => pylon(rig, high, seed)), anchored.point(...at(v), yaw(v - 1, v + 1)));
            }
            for (let v = 0; v + 1 < n; v++) {
                if (!out.owns(...at(v))) continue;
                const length = metresBetween(at(v), at(v + 1)), span = anchored.span(at(v), at(v + 1), end(v) ? low : high, end(v + 1) ? low : high, sag * length);
                for (const side of [-1, 1]) anchored.cable(span, side * T, Math.min(64, Math.max(2, Math.ceil(length / PIECE_M))), CABLE);
                // The carriers: a cable car's two cabins on its first span; gondolas, chairs and T-bars near the eye only.
                const stops = spacing ? Math.max(1, Math.floor(length / spacing)) : v === 0 && ends[0] ? 1 : 0;
                for (let k = 0; k < stops; k++) for (const side of [-1, 1]) {
                    const t = spacing ? (k + (side > 0 ? 0.25 : 0.75)) / stops : side > 0 ? 0.35 : 0.65, livery = rig === 'gondola' ? k % 5 : seed;
                    (spacing ? out.near : out.far).anchored.model(model(`carrier ${rig} ${livery}`, () => carrier(rig, livery)), span, t, side > 0 ? 0 : Math.PI, [side * T, 0, 0]);
                }
            }
        }
    },
} satisfies Kind;
