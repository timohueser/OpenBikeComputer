// Walls as dry-stone walls in the colour of the rock under them, a cross-section that narrows to its top;
// noise barriers as tall thin concrete.
import { GREY } from '../models/rocks';
import { hex } from '../models/shape';
import { STONE } from '../structures/anchored';
import { along, type Kind } from './kind';

/** The width at the foot and the top and the height; a `height` tag wins. Both stand SUNK_M in the ground. */
const DRY = { foot: 0.75, top: 0.35, height: 1.4, colour: GREY | (STONE << 24) };
const NOISE = { foot: 0.3, top: 0.25, height: 3.5, colour: hex('#9c9a94') };
const SUNK_M = 0.3;

export default {
    build(feature, out) {
        const { foot, top, height, colour } = feature.tags.wall === 'noise_barrier' ? NOISE : DRY, h = Math.min(parseFloat(feature.tags.height) || height, 6);
        along(feature, out.near.mesh, out.owns, { corners: [[foot / 2, -SUNK_M], [top / 2, h], [-top / 2, h], [-foot / 2, -SUNK_M]], colour: () => colour, step: 6 });
    },
} satisfies Kind;
