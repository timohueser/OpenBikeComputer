import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { cattle } from './body/cattle';
import { pelt, PINK } from './body/quadruped';

const WHITE = rgb('#e9e6de'), BLACK = rgb('#1e1c1b');

export default {
    name: 'Dairy cattle',
    latin: 'Bos taurus',
    height: 1.45,
    // A black fore and rump with a white girth; a black saddle with a white rump.
    coats: [['..BBBB..', '..BBBBB.', '...BBBB.', '........', '.BBBBBB.', '.BBBBBB.'], ['........', '...BB...', '.BBBBBB.', '.BBBBB..', '....BBB.', '.BBBBBB.']],
    poses: 3,
    build: (s, map, pose) => cattle(s, pose, {
        coat: WHITE, trunk: pelt(map, { B: BLACK }, WHITE), neck: BLACK, ear: BLACK, tuft: WHITE, horns: false,
        head: (d, top) => (d > 0.9 ? PINK : top > 0.5 && d > 0.3 ? WHITE : BLACK),
    }),
} satisfies Animal<string[]>;
