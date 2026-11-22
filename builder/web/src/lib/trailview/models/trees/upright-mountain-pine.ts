import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Upright mountain pine',
  latin: 'Pinus uncinata',
  // A narrow, dark cone of the treeline on one straight stem, leafy nearly to the ground, blunter than a spruce.
  base: 0.1,
  radius: 0.17,
  shape: (t) => 1 - 0.7 * t,
  overlap: 1.1,
  // Whorls of brushes that turn up, as a pine's do.
  droop: -0.2,
  lobes: 4,
  jitter: 0.2,
  whorls: 8,
  tufts: 0.62,
  lift: 0.6,
  trunk: 0.022,
  flare: 1.5,
  // Grey-brown scaly plates, never the orange of a Scots pine.
  bark: ['#3a3430', '#5e554c'],
  needles: {
    winter: ['#1a2a1f', '#2b4230', '#526a48'],
    // Pale new shoots light the tips.
    spring: ['#1b2d20', '#2e4a33', '#6e8c52'],
    summer: ['#1a2c1f', '#2d4832', '#5c7c4c'],
    autumn: ['#1a2b1f', '#2c4631', '#5a784a'],
  },
} satisfies Tree;
