// Crags along mapped aretes and ridges: they give back the knife edge that the terrain rounds off. Their
// places come from hashes of the world point, so every tile and every load puts them in the same place.
import { crag } from '../models/rocks';
import { STONE } from '../structures/anchored';
import { metresPerUnit } from '../terrain/bins';
import type { Kind } from './kind';

/** Crags stand about SPACING_M apart, up to SCATTER_M off the line, and HEIGHT_M tall. */
const SPACING_M = 5, SCATTER_M = 1.5, HEIGHT_M: [number, number] = [1.5, 3.5];
const SHAPES = 4;
const crags = Array.from({ length: SHAPES }, (_, k) => crag(k + 1));

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
                out.place({ x, y, yaw: Math.atan2(-dy, dx) + (r - 0.5) * 0.8, scale: HEIGHT_M[0] + r * (HEIGHT_M[1] - HEIGHT_M[0]), model: crags[Math.floor(hash(x, y, 4) * SHAPES)], flags: STONE, near: true });
            }
        }
    },
} satisfies Kind;
