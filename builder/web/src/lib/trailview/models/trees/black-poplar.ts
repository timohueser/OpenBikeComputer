import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Black poplar',
  latin: 'Populus nigra',
  // A big, leaning, burred bole under a tall, loose dome on heavy limbs that arch out.
  fork: 0.26,
  crown: [0.62, 0.42, 0.4, 0.42],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.2 + 0.8 * t)) ** 0.6,
  clumps: 13,
  open: true,
  dense: 0.95,
  billows: 3,
  bow: 0.4,
  sweep: 0.1,
  flutes: 4,
  trunk: 0.055,
  flare: 1.7,
  bark: ['#3a342e', '#5c544a'],
  leaves: {
    winter: 'bare',
    // The leaves open bronze after red catkins.
    spring: ['#56562a', '#86863c', '#bcb866'],
    summer: ['#283f19', '#435f26', '#75893e'],
    autumn: ['#8a781c', '#baa02c', '#e0ca56'],
  },
  twigs: ['#56503e', '#766e58', '#928a74'],
} satisfies Tree;
