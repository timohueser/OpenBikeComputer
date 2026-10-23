// Crags along mapped aretes and ridges: they give back the knife edge that the terrain rounds off. Their
// places come from hashes of the world point, so every tile and every load puts them in the same place.
import { geometry } from '../models/flat';
import { rock } from '../models/rocks';
import { metresPerUnit } from '../terrain/bins';
import type { Kind } from './kind';

/** Crags stand about SPACING_M apart, up to SCATTER_M off the line, HEIGHT_M tall, and draw to REACH_M. */
const SPACING_M = 5, SCATTER_M = 1.5, HEIGHT_M: [number, number] = [1.5, 3.5], REACH_M = 2000;
const SHAPES = 4;

/** A steady value in [0, 1) for a world point and a salt. */
const hash = (x: number, y: number, salt: number) => {
    const h = Math.sin(x * 12.9898e-3 + y * 78.233e-3 + salt * 37.719) * 43758.5453;
    return h - Math.floor(h);
};

export default {
    build({ parts }, out) {
        for (const { xy } of parts) for (let v = 0; v + 2 < xy.length; v += 2) {
            const [ax, ay, bx, by] = xy.slice(v, v + 4), m = metresPerUnit(ay), length = Math.hypot(bx - ax, by - ay) * m;
            const dx = (bx - ax) / (length || 1), dy = (by - ay) / (length || 1);
            for (let k = 0; k * SPACING_M < length; k++) {
                const d = (k + hash(ax, ay, k)) * SPACING_M;
                if (d >= length) break;
                const across = (hash(ax, ay, k + 0.5) - 0.5) * 2 * SCATTER_M, x = ax + dx * d - dy * across, y = ay + dy * d + dx * across;
                if (!out.owns(x, y)) continue;
                const r = hash(x, y, 3);
                out.place({ x, y, yaw: Math.atan2(-dy, dx) + (r - 0.5) * 0.8, height: HEIGHT_M[0] + r * (HEIGHT_M[1] - HEIGHT_M[0]), model: `${Math.floor(hash(x, y, 4) * SHAPES)}`, tint: 0.85 + 0.3 * hash(x, y, 5) });
            }
        }
    },
    models: Object.fromEntries(Array.from({ length: SHAPES }, (_, k) => [`${k}`, () => ({ tiers: [{ mesh: geometry(rock('crag', k + 1)), until: REACH_M }] })])),
} satisfies Kind;
