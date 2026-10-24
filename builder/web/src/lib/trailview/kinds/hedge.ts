// Hedges: a cross-section as wide as tall with a rounded top, in greens that change along the hedges.
import { pick } from '../models/flat';
import { rgb } from '../structures/mesh';
import { along, type Kind } from './kind';

/** The width, the height and how far it stands in the ground; a `height` tag wins. */
const WIDTH_M = 1.2, HEIGHT_M = 1.8, SUNK_M = 0.3;
const GREENS = ['#3f5a2a', '#4a6430', '#36502a', '#55703a'].map(h => rgb(h));

export default {
    build(feature, out) {
        const h = Math.min(parseFloat(feature.tags.height) || HEIGHT_M, 6), w = WIDTH_M / 2;
        // A segment's green comes from its first vertex, which every tile that holds the segment shares.
        along(feature, out.near.mesh, out.owns, { corners: [[w, -SUNK_M], [w, h - 0.3], [w * 0.6, h], [-w * 0.6, h], [-w, h - 0.3], [-w, -SUNK_M]], colour: (x, y) => pick(GREENS, (x * 7 + y * 13) >>> 4), step: 6 });
    },
} satisfies Kind;
