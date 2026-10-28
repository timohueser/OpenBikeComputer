// Walls as a cross-section that narrows to its top: dry-stone walls, and walls with no `wall` tag, laid in
// stones in the colour of the rock under them; other walls plain grey. Noise barriers as tall thin concrete.
import { rgb } from '../structures/mesh';
import { GREY } from '../models/rocks';
import { STONE } from '../structures/anchored';
import { along, type Kind } from './kind';

/** The width at the foot and the top and the height; a `height` tag wins. Both stand SUNK_M in the ground. */
const DRY = { foot: 0.75, top: 0.35, height: 1.4, colour: GREY | (STONE << 24) };
const PLAIN = { ...DRY, colour: GREY };
const NOISE = { foot: 0.3, top: 0.25, height: 3.5, colour: rgb('#9c9a94') };
const SUNK_M = 0.3;

export default {
    build(feature, out) {
        const wall = feature.tags.wall, { foot, top, height, colour } = wall === 'noise_barrier' ? NOISE : !wall || wall === 'dry_stone' ? DRY : PLAIN, h = Math.min(parseFloat(feature.tags.height) || height, 6);
        along(feature, out.near.mesh, out.owns, { corners: [[foot / 2, -SUNK_M], [top / 2, h], [-top / 2, h], [-foot / 2, -SUNK_M]], colour: () => colour, step: 6 });
    },
} satisfies Kind;
