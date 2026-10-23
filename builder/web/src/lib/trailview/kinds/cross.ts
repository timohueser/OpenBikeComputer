// Crosses: summit crosses and wayside crosses, a model on the surface at each point.
import { cross } from '../models/cross';
import { pointModels, type Kind } from './kind';

/** The height of a summit cross and of a wayside cross; a `height` tag wins. */
const SUMMIT_M = 4, WAYSIDE_M = 2.2;

export default {
    build(feature, out) {
        const { tags } = feature, height = parseFloat(tags.height) || (tags['summit:cross'] === 'yes' ? SUMMIT_M : WAYSIDE_M);
        const model = cross(height, tags.material);
        // A cross faces south across the valley more often than not; the hash of its place turns it a little.
        pointModels(feature, out, (x, y) => ({ yaw: Math.PI / 2 + (((x * 7 + y * 13) % 100) / 100 - 0.5) * 0.6, scale: 1, model }));
    },
} satisfies Kind;
