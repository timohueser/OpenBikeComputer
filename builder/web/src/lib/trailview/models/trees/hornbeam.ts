import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Hornbeam',
  latin: 'Carpinus betulus',
  // A dense egg-shaped crown low on a short, smooth grey bole.
  fork: 0.24,
  crown: [0.56, 0.34, 0.33, 0.46],
  trunk: 0.035,
  flare: 1.3,
  bark: ['#5a5c58', '#8a8c86'],
  leaves: {
    winter: 'bare',
    spring: ['#5e9a3a', '#88c050', '#c2e488'],
    summer: ['#30562a', '#4a7a38', '#76a454'],
    autumn: ['#7a6a1e', '#b49a2e', '#dcc04e'],
  },
  twigs: ['#5a5650', '#78746c', '#948e86'],
} satisfies Tree;
