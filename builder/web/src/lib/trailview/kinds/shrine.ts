// Wayside shrines: a whitewashed pillar with a niche under a small roof, a model on the surface at each point.
import { geometry } from '../models/flat';
import { shrine } from '../models/shrine';
import { pointModels, type Kind } from './kind';

/** A shrine's height without a `height` tag; it draws to REACH_M. */
const HEIGHT_M = 2.8, REACH_M = 1200;

export default {
    build(feature, out) {
        const height = parseFloat(feature.tags.height) || HEIGHT_M;
        // Its niche faces the way past it, which the map does not tell: the hash of its place turns it.
        pointModels(feature, out, (x, y) => ({ yaw: ((x * 7 + y * 13) % 628) / 100, height, model: 'shrine' }));
    },
    models: { shrine: () => ({ tiers: [{ mesh: geometry(shrine()), until: REACH_M }] }) },
} satisfies Kind;
