import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Downy oak',
  latin: 'Quercus pubescens',
  // A small, crooked tree of dry hills: a low, broad crown on a leaning bole.
  fork: 0.2,
  crown: [0.54, 0.5, 0.46, 0.44],
  sweep: 0.15,
  trunk: 0.05,
  flare: 1.4,
  bark: ['#3a3632', '#58524a'],
  // Downy undersides grey the green.
  leaves: {
    winter: 'bare',
    spring: ['#56663a', '#7e9050', '#b0bc84'],
    summer: ['#3a4a2e', '#5a6c44', '#8a9a6c'],
    autumn: ['#6a5430', '#94784a', '#b89a68'],
  },
  twigs: ['#45403a', '#5e5850', '#787068'],
  // Some dry leaves hang on through the winter.
  flecks: { winter: ['#927452', 0.3] },
} satisfies Tree;
