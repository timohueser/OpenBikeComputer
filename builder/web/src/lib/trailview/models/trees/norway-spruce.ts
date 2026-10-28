import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Norway spruce',
  latin: 'Picea abies',
  base: 0.06,
  radius: 0.17,
  shape: (t) => (1 - t) ** 0.95,
  overlap: 1.9,
  droop: 0.16,
  lobes: 7,
  jitter: 0.08,
  trunk: 0.018,
  flare: 1.8,
  bark: ['#4f3426', '#6e4a35'],
  needles: {
    winter: ['#1d3025', '#2d4731', '#4e6a48'],
    spring: ['#21392a', '#335236', '#8cb05a'],
    summer: ['#1f3426', '#324e32', '#5c7a48'],
    autumn: ['#1f3326', '#314c32', '#587646'],
  },
} satisfies Tree;
