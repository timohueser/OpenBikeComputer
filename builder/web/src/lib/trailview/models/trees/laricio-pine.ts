import type { Tree } from '../trees';
import blackPine from './black-pine';

/** The black pine of Corsica, Calabria and Sicily: black pine's needles in a form of its own. */
export default {
  form: 'conifer',
  name: 'Corsican pine',
  latin: 'Pinus nigra subsp. laricio',
  // A very straight trunk, clear for much of its height, under a narrow cone of level whorls.
  base: 0.42,
  radius: 0.15,
  shape: (t) => 1 - 0.6 * t,
  overlap: 0.9,
  droop: -0.2,
  lobes: 4,
  jitter: 0.25,
  tufts: 0.6,
  trunk: 0.016,
  flare: 1.4,
  // Grey plates with a pink cast.
  bark: ['#4a403c', '#9c8682'],
  fade: blackPine.fade,
  needles: blackPine.leaves,
} satisfies Tree;
