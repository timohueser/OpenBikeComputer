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
    winter: ['#3a5a47', '#55805b', '#8cbb82'],
    spring: ['#416950', '#5f9264', '#ccff85'],
    summer: ['#3d6049', '#5d8c5d', '#a3d582'],
    autumn: ['#3d5f49', '#5b885d', '#9ccf7e'],
  },
} satisfies Tree;
