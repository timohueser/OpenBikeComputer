// Paths: the terrain draws a path itself. A via ferrata has no heights, so it draws as a steel cable
// close over the drawn surface, sagging a little between its anchor bolts. The cable is a pixel wide at
// any distance, so that its line shows on the face from the valley; the bolts draw only near the eye. A
// via ferrata's bridge is left out: its cable would follow the ground under it.
import { Model } from '../models/flat';
import { metresBetween } from '../structures/anchored';
import { rgb } from '../structures/mesh';
import type { Kind } from './kind';

/** The cable's height over the drawn surface at a bolt, its sag between two, the most metres between two bolts, and its diameter. */
const CABLE_M = 0.25, SAG_M = 0.03, SPACING_M = 3, DIAMETER_M = 0.016;
const STEEL = rgb('#5a5e62');
/** An anchor bolt, hung from the cable down to a plate on the ground. */
const BOLT = new Model().box([0, 0, -CABLE_M / 2], [0.02, 0.02, CABLE_M], STEEL).box([0, 0, -CABLE_M], [0.06, 0.06, 0.03], STEEL);

export default {
    build({ tags, parts }, out) {
        if (tags.highway !== 'via_ferrata' || (tags.bridge && tags.bridge !== 'no')) return;
        const { anchored } = out.far;
        for (const { xy } of parts) {
            const n = xy.length;
            for (let v = 0; v + 3 < n; v += 2) {
                if (!out.owns(xy[v], xy[v + 1])) continue;
                const a: [number, number] = [xy[v], xy[v + 1]], b: [number, number] = [xy[v + 2], xy[v + 3]];
                const pieces = Math.max(1, Math.ceil(metresBetween(a, b) / SPACING_M));
                const at = (k: number): [number, number] => [a[0] + (b[0] - a[0]) * k / pieces, a[1] + (b[1] - a[1]) * k / pieces];
                for (let k = 0; k < pieces; k++) {
                    const span = anchored.span(at(k), at(k + 1), CABLE_M, CABLE_M, SAG_M);
                    anchored.cable(span, 0, STEEL, DIAMETER_M);
                    out.near.anchored.model(BOLT, span);
                }
            }
            // The node that owns the last vertex, an end of the line, sets its bolt.
            if (n >= 4 && out.owns(xy[n - 2], xy[n - 1])) out.near.anchored.model(BOLT, anchored.point(xy[n - 2], xy[n - 1], 0), 0, 0, [0, 0, CABLE_M]);
        }
    },
} satisfies Kind;
