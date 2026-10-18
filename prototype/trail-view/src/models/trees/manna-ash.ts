import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Manna ash',
  latin: 'Fraxinus ornus',
  // A small, rounded tree, often on two stems, with smooth grey bark.
  fork: 0.3,
  crown: [0.6, 0.4, 0.38, 0.38],
  stems: 2,
  trunk: 0.03,
  flare: 1.3,
  bark: ['#5e605a', '#8e928a'],
  leaves: {
    winter: 'bare',
    spring: ['#4a7a30', '#76a648', '#acd078'],
    summer: ['#2e5226', '#487836', '#76a052'],
    autumn: ['#56383a', '#80524a', '#a8785e'],
  },
  twigs: ['#5e5e58', '#7e7e76', '#9a9a92'],
  // Plumes of cream-white flowers cover the crown in spring.
  flecks: { spring: ['#f4f0dc', 0.4] },
} satisfies Tree;
