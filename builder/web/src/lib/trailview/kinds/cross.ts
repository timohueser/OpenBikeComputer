// Crosses: summit crosses and wayside crosses, a model on the surface at each point.
import { geometry } from '../models/flat';
import { cross, MATERIALS } from '../models/cross';
import { pointModels, type Kind } from './kind';

/** The height of a summit cross and of a wayside cross; a `height` tag wins. A cross draws to REACH_M. */
const SUMMIT_M = 4, WAYSIDE_M = 2.2, REACH_M = 1500;

export default {
    build({ tags, parts }, out) {
        const height = parseFloat(tags.height) || (tags['summit:cross'] === 'yes' ? SUMMIT_M : WAYSIDE_M), model = tags.material in MATERIALS ? tags.material : 'wood';
        // A cross faces south across the valley more often than not; the hash of its place turns it a little.
        pointModels({ tags, parts }, out, (x, y) => ({ yaw: (((x * 7 + y * 13) % 100) / 100 - 0.5) * 0.6, height, model }));
    },
    models: Object.fromEntries(Object.keys(MATERIALS).map(name => [name, () => ({ tiers: [{ mesh: geometry(cross(name)), until: REACH_M }] })])),
} satisfies Kind;
