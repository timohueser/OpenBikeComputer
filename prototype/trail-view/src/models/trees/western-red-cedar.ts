import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Western red cedar',
  latin: 'Thuja plicata',
  base: 0.1,
  radius: 0.24,
  shape: (t) => (1 - t) ** 1.3,
  overlap: 1.8,
  droop: 0.34,
  lobes: 9,
  jitter: 0.12,
  trunk: 0.03,
  // The buttressed foot of an old cedar.
  flare: 2.6,
  bark: ['#5e2f1e', '#8a4e34'],
  needles: {
    winter: ['#2a4a2e', '#40683c', '#6f9050'],
    spring: ['#2e5432', '#4a7a40', '#a4c462'],
    summer: ['#2d5131', '#47763f', '#8db458'],
    autumn: ['#2c4f30', '#45733e', '#86ad54'],
  },
} satisfies Tree;
