import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Dragon tree',
  latin: 'Dracaena draco',
  fork: 0.4,
  crown: [0.76, 0.46, 0.46, 0.28],
  branching: [3, 3, 3],
  trunk: 0.08,
  // The trunk swells towards its crown.
  neck: 1.05,
  flare: 1.5,
  bark: ['#6a655a', '#9a968a'],
  leaves: {
    winter: ['#2e4636', '#4f6e52', '#9ab486'],
    spring: ['#2f4837', '#527254', '#a0ba8a'],
    summer: ['#2e4636', '#4f6e52', '#9ab486'],
    autumn: ['#2e4636', '#4f6e52', '#9ab486'],
  },
  twigs: ['#6e6a60', '#8e8a7e', '#a8a498'],
} satisfies Tree;
