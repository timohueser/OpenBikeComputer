// Buildings show their use, which the bake gives them (specs/trailview-features.md): a sign at the corner of the
// wall that faces the nearest way. The houses draw the buildings themselves.
import { hospitalSign, pharmacySign, shopSign } from '../models/amenities';
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
        if (!sign || !ring || ring.length < 6) return;
        const n = ring.length / 2, unit = metresPerUnit(ring[1]);
        const cx = ring.filter((_, i) => i % 2 === 0).reduce((a, b) => a + b) / n, cy = ring.filter((_, i) => i % 2).reduce((a, b) => a + b) / n;
        // The wall nearest a way, else the longest.
        let best: { x: number; y: number; ox: number; oy: number; score: number } | undefined;
        for (let i = 0; i < n; i++) {
            const ax = ring[2 * i], ay = ring[2 * i + 1], bx = ring[(2 * i + 2) % ring.length], by = ring[(2 * i + 3) % ring.length];
            const length = Math.hypot(bx - ax, by - ay);
            if (length * unit < WALL_M) continue;
            const mx = (ax + bx) / 2, my = (ay + by) / 2, way = nearestWay(out.ways, mx, my);
            // Out of the building: across the wall, away from its middle.
            const side = (by - ay) * (mx - cx) - (bx - ax) * (my - cy) > 0 ? 1 : -1, ox = side * (by - ay) / length, oy = -side * (bx - ax) / length;
            const score = way ? Math.hypot(way.px, way.py) * unit : 1e6 - length * unit;
            if (best && score >= best.score) continue;
            // The corner nearer the way, and CORNER_M from it along the wall.
            const [wx, wy] = way ? [mx + way.px, my + way.py] : [ax, ay], fromA = Math.hypot(wx - ax, wy - ay) <= Math.hypot(wx - bx, wy - by);
            const [x, y] = fromA ? [ax, ay] : [bx, by], along = (fromA ? 1 : -1) * CORNER_M / unit / length;
            best = { x: x + (bx - ax) * along, y: y + (by - ay) * along, ox, oy, score };
        }
        if (!best) return;
        const x = best.x + best.ox * 0.1 / unit, y = best.y + best.oy * 0.1 / unit;
        if (out.owns(x, y)) out.place({ x, y, yaw: Math.atan2(best.ox, best.oy), height: sign.height, model: tags.use });
    },
    models: Object.fromEntries(Object.entries(SIGNS).map(([name, { spec }]) => [name, spec])),
} satisfies Kind;
