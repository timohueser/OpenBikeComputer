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
    winter: ['#1f3a33', '#2d5444', '#557a56'],
    spring: ['#24453a', '#33664b', '#9cc35c'],
    summer: ['#22433a', '#33664b', '#6f9c58'],
    autumn: ['#22413a', '#31604a', '#638f52'],
  },
} satisfies Tree;
