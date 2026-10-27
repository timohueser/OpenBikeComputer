import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Portuguese oak',
  latin: 'Quercus faginea',
  // A small tree with a short bole under a dense, rounded crown.
  fork: 0.3,
  crown: [0.6, 0.44, 0.42, 0.4],
  sweep: 0.08,
  trunk: 0.045,
  flare: 1.4,
  // Grey bark cracked into small squares.
  bark: ['#55504a', '#827c74'],
  // Small, stiff leaves, grey-green and felted beneath.
  leaves: {
    winter: 'bare',
    spring: ['#566a44', '#82966a', '#b8c49c'],
    summer: ['#3a482e', '#5e6e4a', '#909e74'],
    autumn: ['#6a5a30', '#8e7c46', '#b4a068'],
  },
  twigs: ['#4a4642', '#625c56', '#7c766e'],
  // Pale, tawny dead leaves hang on through the winter.
  flecks: { winter: ['#a8865a', 0.6] },
} satisfies Tree;
