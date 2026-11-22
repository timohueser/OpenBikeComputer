import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Grey willow',
  latin: 'Salix cinerea',
  fork: 0.12,
  // A dense dome of many rods, wider than tall and leafy to the ground.
  crown: [0.48, 0.62, 0.56, 0.46],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.35 + 0.65 * t)) ** 0.45,
  clumps: 13,
  billows: 4,
  bow: 0.3,
  stems: 4,
  leaders: true,
  heads: { count: 110, width: 0.028, height: 1.6 },
  trunk: 0.022,
  flare: 1.2,
  bark: ['#4a4038', '#6e6458'],
  // Dull grey-green leaves, felted grey beneath.
  leaves: {
    winter: 'bare',
    spring: ['#4a5a36', '#74865a', '#aab88e'],
    summer: ['#353d26', '#575d3e', '#898768'],
    autumn: ['#56582e', '#7c7a44', '#a8a066'],
  },
  twigs: ['#4a3e36', '#665850', '#82746a'],
  // Pussy-willow catkins turn the bush silver and yellow before the leaves.
  flecks: { spring: ['#e2d466', 0.35] },
} satisfies Tree;
