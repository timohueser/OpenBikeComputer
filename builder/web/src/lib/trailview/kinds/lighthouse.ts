// Lighthouses: a white tower with its lantern on the middle of the footprint, standing on the drawn surface. The
// lantern's glass shines day and night.
import { lighthouse, lighthouseLight } from '../models/landmarks';
import { LAMP } from '../structures/mesh';
import { footprint, type Kind } from './kind';

/** A storey's height, and the tower's without tags. */
const STOREY_M = 2.8, HEIGHT_M = 7;

export default {
    build({ tags, parts }, out) {
        const ring = parts.find(part => part.outer)?.xy;
        if (!ring) return;
        const fit = footprint(ring);
        if (!out.owns(fit.x, fit.y)) return;
        const height = parseFloat(tags.height) || (parseFloat(tags['building:levels']) * STOREY_M || HEIGHT_M);
        const anchor = out.far.anchored.point(fit.x, fit.y, fit.angle);
        out.far.anchored.model(lighthouse(fit.width, fit.depth, height), anchor);
        out.far.anchored.model(lighthouseLight(fit.width, fit.depth, height), anchor, 0, 0, [0, 0, 0], 1, LAMP);
    },
} satisfies Kind;
