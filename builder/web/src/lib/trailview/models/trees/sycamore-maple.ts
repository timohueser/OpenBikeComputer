import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Sycamore maple',
  latin: 'Acer pseudoplatanus',
  // A broad dome of few, heavy billows on a stout bole.
  fork: 0.32,
  crown: [0.6, 0.42, 0.42, 0.38],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.22 + 0.78 * t)) ** 0.6,
  clumps: 11,
  billows: 3,
  bow: 0.1,
  trunk: 0.05,
  flare: 1.5,
  bark: ['#5f564d', '#8a7e70'],
  leaves: {
    winter: 'bare',
    spring: ['#51713e', '#6c8d4d', '#95b254'],
    summer: ['#324f21', '#496a2b', '#718b3f'],
    autumn: ['#7a5a26', '#b08a36', '#dcb752'],
  },
  twigs: ['#6e625a', '#8e8076', '#a89a8e'],
} satisfies Tree;
