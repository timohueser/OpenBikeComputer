import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Black poplar',
  latin: 'Populus nigra',
  // A big, leaning bole under a tall dome, narrower than an oak's.
  fork: 0.26,
  crown: [0.62, 0.42, 0.4, 0.42],
  sweep: 0.1,
  trunk: 0.055,
  flare: 1.7,
  bark: ['#3a342e', '#5c544a'],
  leaves: {
    winter: 'bare',
    // The leaves open bronze after red catkins.
    spring: ['#56562a', '#86863c', '#bcb866'],
    summer: ['#2c4c22', '#487232', '#7ea450'],
    autumn: ['#8a781c', '#baa02c', '#e0ca56'],
  },
  twigs: ['#56503e', '#766e58', '#928a74'],
} satisfies Tree;
