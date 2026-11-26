import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Sweet chestnut',
  latin: 'Castanea sativa',
  // A massive bole under a broad, glossy dome.
  fork: 0.3,
  crown: [0.6, 0.48, 0.46, 0.42],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.2 + 0.8 * t)) ** 0.6,
  clumps: 12,
  billows: 3,
  heads: { count: 500, width: 0.07, height: 0.3, points: 4 },
  flutes: 5,
  trunk: 0.075,
  flare: 1.5,
  bark: ['#4e4238', '#7a6a5a'],
  leaves: {
    winter: 'bare',
    spring: ['#4e6e26', '#7a9e36', '#b4d060'],
    summer: ['#2a3d16', '#445c20', '#7c8d31'],
    autumn: ['#7a5a1c', '#b08a2a', '#d8b44a'],
  },
  twigs: ['#5a4a3e', '#76665a', '#8e7e70'],
  // Bursts of cream catkins stand all over the crown in early summer.
  flecks: { summer: ['#e2d690', 0.12] },
} satisfies Tree;
