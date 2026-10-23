import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Hawthorn, wind-bent',
  latin: 'Crataegus monogyna',
  fork: 0.3,
  crown: [0.55, 0.6, 0.42, 0.24],
  // Leans downwind of the Atlantic westerlies.
  sweep: 0.8,
  keepsHeading: true,
  trunk: 0.06,
  flare: 1.3,
  bark: ['#4a3d36', '#6a5a50'],
  leaves: {
    winter: 'bare',
    spring: ['#3a5e2c', '#557d3a', '#86a85a'],
    summer: ['#2f5226', '#456e33', '#6c9448'],
    autumn: ['#5a5a2e', '#7a7038', '#9a8a4a'],
  },
  twigs: ['#4c3f3a', '#665650', '#80706a'],
  flecks: { spring: ['#f6f1e6', 0.55], autumn: ['#a3242a', 0.35] },
} satisfies Tree;
