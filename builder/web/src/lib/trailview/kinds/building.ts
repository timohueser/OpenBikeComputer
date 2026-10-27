// Buildings show their use, which the bake gives them (specs/trailview-features.md): a sign at the corner of the
// wall that faces the nearest way. The houses draw the buildings themselves.
import { hospitalSign, pharmacySign, shopSign } from '../models/amenities';
import { SKIPPED } from '../structures/house-records';
import { metresPerUnit } from '../terrain/bins';
import { nearestWay, unitModel, type Kind } from './kind';

/**
 * A sign hangs on a wall at least WALL_M long, CORNER_M from its corner nearer the way: closer than a
 * window or a shutter of any house of houses.wgsl comes to it, and far from the door in the wall's
 * middle. It draws to REACH_M.
 */
const WALL_M = 2.5, CORNER_M = 0.12, REACH_M = 500;
const SIGNS = { shop: unitModel(shopSign, REACH_M), pharmacy: unitModel(pharmacySign, REACH_M), hospital: unitModel(hospitalSign, REACH_M) };

export default {
    build({ tags, parts }, out) {
        const sign = SIGNS[tags.use as keyof typeof SIGNS], ring = parts.find(part => part.outer)?.xy;
        if (!sign || !ring || ring.length < 6 || SKIPPED.has(tags.building)) return;
        // The node that owns the sign's corner places it: the others need not look for the way.
        const n = ring.length / 2, unit = metresPerUnit(ring[1]);
        if (!ring.some((_, i) => i % 2 === 0 && out.owns(ring[i], ring[i + 1]))) return;
        // A wall's outside is (dy, −dx) along it when the ring's signed area is positive, else the other side.
        let twice = 0;
        for (let i = 0; i < n; i++) {
            const j = (i + 1) % n;
            twice += (ring[2 * i] - ring[0]) * (ring[2 * j + 1] - ring[1]) - (ring[2 * j] - ring[0]) * (ring[2 * i + 1] - ring[1]);
        }
        const side = twice > 0 ? 1 : -1;
        // The wall nearest a way, else the longest.
        let best: { corner: [number, number]; x: number; y: number; ox: number; oy: number; score: number } | undefined;
        for (let i = 0; i < n; i++) {
            const ax = ring[2 * i], ay = ring[2 * i + 1], bx = ring[(2 * i + 2) % ring.length], by = ring[(2 * i + 3) % ring.length];
            const length = Math.hypot(bx - ax, by - ay);
            if (length * unit < WALL_M) continue;
            const mx = (ax + bx) / 2, my = (ay + by) / 2, way = nearestWay(out.ways, mx, my);
            const score = way ? Math.hypot(way.px, way.py) * unit : 1e6 - length * unit;
            if (best && score >= best.score) continue;
            // The corner nearer the way, and CORNER_M from it along the wall.
            const [wx, wy] = way ? [mx + way.px, my + way.py] : [ax, ay], fromA = Math.hypot(wx - ax, wy - ay) <= Math.hypot(wx - bx, wy - by);
            const corner: [number, number] = fromA ? [ax, ay] : [bx, by], along = (fromA ? 1 : -1) * CORNER_M / unit / length;
            best = { corner, x: corner[0] + (bx - ax) * along, y: corner[1] + (by - ay) * along, ox: side * (by - ay) / length, oy: -side * (bx - ax) / length, score };
        }
        if (!best || !out.owns(...best.corner)) return;
        const x = best.x + best.ox * 0.1 / unit, y = best.y + best.oy * 0.1 / unit;
        out.place({ x, y, yaw: Math.atan2(best.ox, best.oy), height: sign.height, model: tags.use });
    },
    models: Object.fromEntries(Object.entries(SIGNS).map(([name, { spec }]) => [name, spec])),
} satisfies Kind;
