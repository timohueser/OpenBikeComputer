import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Sweet chestnut',
  latin: 'Castanea sativa',
  // A massive bole under a broad, glossy dome.
  fork: 0.3,
  crown: [0.6, 0.48, 0.46, 0.42],
  trunk: 0.075,
  flare: 1.5,
  bark: ['#4e4238', '#7a6a5a'],
  leaves: {
    winter: 'bare',
    spring: ['#4e6e26', '#7a9e36', '#b4d060'],
    summer: ['#2e4a1e', '#4a6e2a', '#86a840'],
    autumn: ['#7a5a1c', '#b08a2a', '#d8b44a'],
  },
  twigs: ['#5a4a3e', '#76665a', '#8e7e70'],
  // Cream catkins cover the crown in early summer.
  flecks: { summer: ['#e2d690', 0.12] },
} satisfies Tree;
