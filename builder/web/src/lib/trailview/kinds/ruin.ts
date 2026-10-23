// Ruins: a roofless stone tower on the rectangle of the footprint, standing on the drawn surface.
import { ruin } from '../models/landmarks';
import { footprint, type Kind } from './kind';

export default {
    build({ tags, parts }, out) {
        const ring = parts.find(part => part.outer)?.xy;
        if (!ring) return;
        const fit = footprint(ring);
        if (!out.owns(fit.x, fit.y) || fit.width < 1 || fit.depth < 1) return;
        const area = fit.width * fit.depth, height = parseFloat(tags.height) || Math.min(14, 6 + 0.6 * Math.sqrt(area));
        out.far.anchored.model(ruin(fit.width, fit.depth, height, (ring[0] ^ ring[1]) & 0xffff), out.far.anchored.point(fit.x, fit.y, fit.angle + Math.PI / 2));
    },
} satisfies Kind;
