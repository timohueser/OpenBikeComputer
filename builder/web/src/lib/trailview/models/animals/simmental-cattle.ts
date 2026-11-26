import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { cattle } from './body/cattle';
import { pelt, PINK } from './body/quadruped';

const WHITE = rgb('#ece6da');
// A red back over a white flank and a white chest.
const MAP = ['.RRRRRR.', 'RRRRRRRR', '..RRRRRR', '.RRRRRR.', 'RRRRRRRR', '.RRRRRR.'];

export default {
    name: 'Simmental cattle',
    latin: 'Bos taurus',
    height: 1.45,
    coats: ['#a85428', '#c88e4c'].map(rgb),
    poses: 3,
    build: (s, red, pose) => cattle(s, pose, { coat: red, trunk: pelt(MAP, { R: red }, WHITE), shin: WHITE, tuft: WHITE, head: (d, top) => (d > 0.9 ? PINK : d < 0.2 && top < 0.5 ? red : WHITE) }),
} satisfies Animal<number>;
