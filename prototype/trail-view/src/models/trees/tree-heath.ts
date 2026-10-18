import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Tree heath',
  latin: 'Erica arborea',
  fork: 0.15,
  // Upright plumes from a few stems, taller than wide.
  crown: [0.55, 0.3, 0.3, 0.5],
  stems: 4,
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
