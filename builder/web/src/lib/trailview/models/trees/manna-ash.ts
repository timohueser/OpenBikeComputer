import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Manna ash',
  latin: 'Fraxinus ornus',
  // A small, rounded tree, often on two stems, with smooth grey bark.
  fork: 0.3,
  crown: [0.6, 0.4, 0.38, 0.38],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.2 + 0.8 * t)) ** 0.6,
  clumps: 11,
  billows: 3,
  heads: { count: 90, width: 0.05, height: 1.2 },
  stems: 2,
  leaders: true,
  trunk: 0.03,
  flare: 1.3,
  bark: ['#5e605a', '#8e928a'],
  leaves: {
    winter: 'bare',
    spring: ['#4a7a30', '#76a648', '#acd078'],
    summer: ['#2a441d', '#43642a', '#6d8640'],
    autumn: ['#56383a', '#80524a', '#a8785e'],
  },
  twigs: ['#5e5e58', '#7e7e76', '#9a9a92'],
  // Plumes of cream-white flowers cover the crown in spring.
  flecks: { spring: ['#f4f0dc', 0.4] },
} satisfies Tree;
