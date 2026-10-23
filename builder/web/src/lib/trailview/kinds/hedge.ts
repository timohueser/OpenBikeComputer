// Hedges: a cross-section as wide as tall with a rounded top, in greens that change from hedge to hedge.
import { hex, pick } from '../models/shape';
import { along, type Kind } from './kind';

/** The width, the height and how far it stands in the ground; a `height` tag wins. */
const WIDTH_M = 1.2, HEIGHT_M = 1.8, SUNK_M = 0.3;
const GREENS = ['#3f5a2a', '#4a6430', '#36502a', '#55703a'].map(hex);

export default {
    build(feature, out) {
        const h = Math.min(parseFloat(feature.tags.height) || HEIGHT_M, 6), w = WIDTH_M / 2;
        const green = pick(GREENS, Math.abs(feature.parts[0]?.xy[0] ?? 0) >> 4);
        along(feature, out.near.mesh, out.owns, { corners: [[w, -SUNK_M], [w, h - 0.3], [w * 0.6, h], [-w * 0.6, h], [-w, h - 0.3], [-w, -SUNK_M]], colour: () => green, step: 6 });
    },
} satisfies Kind;
