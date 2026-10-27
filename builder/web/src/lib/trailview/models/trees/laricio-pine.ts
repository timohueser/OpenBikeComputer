import type { Tree } from '../trees';
import blackPine from './black-pine';

/** The black pine of Corsica, Calabria and Sicily: black pine's needles in a form of its own. */
export default {
  form: 'conifer',
  name: 'Corsican pine',
  latin: 'Pinus nigra subsp. laricio',
  // A very straight trunk, clear for most of its height, under a narrow cone of level whorls.
  base: 0.52,
  radius: 0.14,
  shape: (t) => (1 - t) ** 0.7,
  overlap: 0.9,
  droop: 0.15,
  lobes: 4,
  jitter: 0.25,
  tufts: true,
  trunk: 0.016,
  flare: 1.4,
  // Grey plates with a pink cast.
  bark: ['#4a403c', '#9c8682'],
  fade: blackPine.fade,
  needles: blackPine.leaves,
} satisfies Tree;
