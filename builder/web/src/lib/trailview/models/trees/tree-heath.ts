import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Tree heath',
  latin: 'Erica arborea',
  fork: 0.1,
  // Feathery plumes on many rods from one stool, taller than wide.
  crown: [0.5, 0.36, 0.34, 0.5],
  dense: 0.95,
  rods: 18,
  heads: { count: 110, width: 0.022, height: 2.5 },
  trunk: 0.02,
  flare: 1.2,
  bark: ['#4a3a32', '#6e5848'],
  leaves: {
    winter: ['#2c4422', '#486630', '#7e9a50'],
    spring: ['#2e4824', '#4c6c32', '#86a456'],
    summer: ['#2d4623', '#4a6a31', '#82a054'],
    autumn: ['#2c4522', '#496831', '#7e9c52'],
  },
  twigs: ['#4a3a32', '#5e4a3e', '#6e5848'],
  // White bells cover the whole bush in spring.
  flecks: { spring: ['#f4f0ea', 0.7] },
} satisfies Tree;
