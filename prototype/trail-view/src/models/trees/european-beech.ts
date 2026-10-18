import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'European beech',
  latin: 'Fagus sylvatica',
  fork: 0.4,
  crown: [0.66, 0.34, 0.34, 0.34],
  trunk: 0.04,
  flare: 1.4,
  bark: ['#6f716c', '#a2a29a'],
  leaves: {
    // Beech holds its dead leaves through the winter.
    winter: ['#6a4026', '#8a5a34', '#a8764a'],
    spring: ['#6aa63e', '#94c855', '#c9e98a'],
    summer: ['#335c28', '#4a7c35', '#73a24c'],
    autumn: ['#7e3a18', '#b45e26', '#e08a3c'],
  },
  twigs: ['#5e5a56', '#7c7670', '#97908a'],
} satisfies Tree;
