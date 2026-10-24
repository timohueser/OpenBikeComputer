// Cable cars and ski lifts: a station at each end of the line, a pylon at each vertex between (the bake
// adds the pylons that OSM lacks), two cables that sag from anchor to anchor, and carriers on them.
import { CABLE_AT_STATION_M, carrier, pylon, station, TRACK_HALF_M, type Rig } from '../models/aerialways';
import type { Model } from '../models/flat';
import { metresBetween } from '../structures/anchored';
import { rgb } from '../structures/mesh';
import type { Kind } from './kind';

/** As RIGS of tools/trailview/lifts.py: the cable's height over the ground at a pylon, and its sag as a share of the span; and the spacing of the carriers, 0 for a cable car's two cabins. */
const RIGS: Record<Rig, { pylon: number; sag: number; spacing: number }> = {
    cable_car: { pylon: 30, sag: 0.04, spacing: 0 },
    gondola: { pylon: 14, sag: 0.02, spacing: 70 },
    chair_lift: { pylon: 10, sag: 0.02, spacing: 30 },
    drag_lift: { pylon: 7, sag: 0.01, spacing: 15 },
};
const CLASSES: Record<string, Rig> = {
    cable_car: 'cable_car', gondola: 'gondola', mixed_lift: 'gondola', chair_lift: 'chair_lift', drag_lift: 'drag_lift', 't-bar': 'drag_lift', 'j-bar': 'drag_lift', platter: 'drag_lift',
};
const CABLE = rgb('#2b2b2b');
/** Seeds of a lift's livery: a multiple of the counts of the colours of models/aerialways.ts. */
const SEEDS = 30;

const models = new Map<string, Model>();
const model = (name: string, make: () => Model) => models.get(name) ?? models.set(name, make()).get(name)!;

/** A steady seed for a text (FNV-1a). */
const seedOf = (text: string) => [...text].reduce((h, c) => Math.imul(h ^ c.charCodeAt(0), 16777619) >>> 0, 2166136261) % SEEDS;

export default {
    build({ tags, parts }, out) {
        const rig = CLASSES[tags.aerialway], { anchored } = out.far;
        if (!rig) return;
        const { pylon: high, sag, spacing } = RIGS[rig], low = CABLE_AT_STATION_M[rig], T = TRACK_HALF_M[rig];
        // One livery per lift, from what each tile of it holds: its class and its name.
        const seed = seedOf(`${tags.aerialway} ${tags.name ?? ''}`);
        // The bake's `stations` names the ends that have a station of their own; another end shares the station of another lift.
        const own = [!tags.stations || tags.stations === 'start', !tags.stations || tags.stations === 'end'];
        for (const { xy } of parts) {
            const n = xy.length / 2, at = (v: number): [number, number] => [xy[2 * v], xy[2 * v + 1]];
            // The part's first or last vertex ends the lift where the node owns it, and so does the far end of an owned span.
            const end = (v: number) => v === 0 || v === n - 1;
            const yaw = (from: number, to: number) => Math.atan2(xy[2 * from + 1] - xy[2 * to + 1], xy[2 * to] - xy[2 * from]);
            for (let v = 0; v < n; v++) {
                if (!out.owns(...at(v))) continue;
                if (!end(v)) {
                    // A pylon stays in the anchored mesh: its foot is the end of its cables' spans.
                    anchored.model(model(`pylon ${rig} ${seed}`, () => pylon(rig, high, seed)), anchored.point(...at(v), yaw(v - 1, v + 1)));
                } else if (own[v ? 1 : 0]) {
                    anchored.model(model(`station ${rig} ${seed}`, () => station(rig, seed)), anchored.point(...at(v), v ? yaw(v, v - 1) : yaw(v, v + 1)));
                }
            }
            for (let v = 0; v + 1 < n; v++) {
                if (!out.owns(...at(v))) continue;
                const length = metresBetween(at(v), at(v + 1)), span = anchored.span(at(v), at(v + 1), end(v) ? low : high, end(v + 1) ? low : high, sag * length);
                for (const side of [-1, 1]) anchored.cable(span, side * T, CABLE);
                // The carriers: a cable car's two cabins on its first span; gondolas, chairs and T-bars near the eye only.
                const stops = spacing ? Math.max(1, Math.floor(length / spacing)) : v === 0 ? 1 : 0;
                for (let k = 0; k < stops; k++) for (const side of [-1, 1]) {
                    const t = spacing ? (k + (side > 0 ? 0.25 : 0.75)) / stops : side > 0 ? 0.35 : 0.65, livery = rig === 'gondola' ? k % 5 : seed;
                    (spacing ? out.near : out.far).anchored.model(model(`carrier ${rig} ${livery}`, () => carrier(rig, livery)), span, t, side > 0 ? 0 : Math.PI, [side * T, 0, 0]);
                }
            }
        }
    },
} satisfies Kind;
