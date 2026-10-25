// Lighthouses: a white tower with its lantern on the middle of the footprint, standing on the drawn surface.
import { lighthouse } from '../models/landmarks';
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
        out.far.anchored.model(lighthouse(fit.width, fit.depth, height), out.far.anchored.point(fit.x, fit.y, fit.angle));
    },
} satisfies Kind;
