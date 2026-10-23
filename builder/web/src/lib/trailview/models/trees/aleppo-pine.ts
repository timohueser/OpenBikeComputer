import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Aleppo pine',
  latin: 'Pinus halepensis',
  fork: 0.5,
  crown: [0.76, 0.42, 0.42, 0.26],
  open: true,
  // A bent, leaning trunk; it takes a random heading like any other tree.
  sweep: 0.22,
  trunk: 0.035,
  flare: 1.4,
  bark: ['#5a4038', '#8a6a5c'],
  leaves: {
    winter: ['#46573f', '#677a58', '#94a67c'],
    spring: ['#4a5e42', '#6f8460', '#a8bb88'],
    summer: ['#4a5c42', '#6d825e', '#a3b585'],
    autumn: ['#48593f', '#6b7f5b', '#9eaf80'],
  },
  twigs: ['#5a4038', '#7a5a4c', '#8a6a5c'],
} satisfies Tree;
