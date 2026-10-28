import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Upright mountain pine',
  latin: 'Pinus uncinata',
  // A narrow, dark cone of the treeline on one straight stem, leafy to the ground, blunter than a spruce.
  base: 0.04,
  radius: 0.16,
  shape: (t) => 1 - 0.5 * t,
  overlap: 1.7,
  // Branch tips turn up, as a pine's do, so the tiers' rims rise instead of hanging.
  droop: -0.08,
  lobes: 5,
  jitter: 0.3,
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
