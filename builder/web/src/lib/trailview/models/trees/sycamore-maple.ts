import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Sycamore maple',
  latin: 'Acer pseudoplatanus',
  fork: 0.32,
  crown: [0.6, 0.42, 0.42, 0.38],
  trunk: 0.05,
  flare: 1.5,
  bark: ['#5f564d', '#8a7e70'],
  leaves: {
    winter: 'bare',
    spring: ['#4f7a35', '#78a84a', '#b2d070'],
    summer: ['#365f2c', '#4f7f38', '#7aa651'],
    autumn: ['#7a5a26', '#b08a36', '#dcb752'],
  },
  twigs: ['#6e625a', '#8e8076', '#a89a8e'],
} satisfies Tree;
