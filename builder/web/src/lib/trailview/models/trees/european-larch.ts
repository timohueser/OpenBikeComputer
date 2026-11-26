import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'European larch',
  latin: 'Larix decidua',
  base: 0.08,
  radius: 0.22,
  shape: (t) => (1 - t) ** 0.7,
  boughs: 44,
  overlap: 0.95,
  droop: 0.35,
  lobes: 5,
  jitter: 0.25,
  trunk: 0.017,
  neck: 0.3,
  flare: 1.7,
  bark: ['#5e3a28', '#7a4e36'],
  needles: {
    winter: 'bare',
    spring: ['#5f9447', '#8fc25e', '#c6e58d'],
    summer: ['#4b7a3c', '#6d9e4c', '#a5c96f'],
    autumn: ['#9a6a24', '#c99632', '#ecc458'],
  },
  // Straw-yellow young twigs.
  twigs: ['#6a5844', '#96805e', '#c0aa80'],
} satisfies Tree;
