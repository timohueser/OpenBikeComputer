import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'European larch',
  latin: 'Larix decidua',
  base: 0.08,
  radius: 0.24,
  shape: (t) => (1 - t) ** 0.85,
  overlap: 0.95,
  droop: 0.35,
  lobes: 5,
  jitter: 0.25,
  trunk: 0.017,
  flare: 1.7,
  bark: ['#5e3a28', '#7a4e36'],
  needles: {
    winter: 'bare',
    spring: ['#5f9447', '#8fc25e', '#c6e58d'],
    summer: ['#4b7a3c', '#6d9e4c', '#a5c96f'],
    autumn: ['#9a6a24', '#c99632', '#ecc458'],
  },
  twigs: ['#5e5048', '#7d6c60', '#9a887a'],
} satisfies Tree;
