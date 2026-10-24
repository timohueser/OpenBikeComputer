import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Douglas fir',
  latin: 'Pseudotsuga menziesii',
  base: 0.3,
  radius: 0.18,
  shape: (t) => (1 - t) ** 0.8,
  overlap: 1.7,
  droop: 0.14,
  lobes: 6,
  jitter: 0.3,
  trunk: 0.022,
  flare: 1.6,
  bark: ['#3e271c', '#5a3a2a'],
  needles: {
    winter: ['#1e3a33', '#2c5446', '#4d7458'],
    spring: ['#22443a', '#34654e', '#86b06a'],
    summer: ['#20423a', '#33624d', '#6a9460'],
    autumn: ['#20403a', '#325f4b', '#64905c'],
  },
} satisfies Tree;
