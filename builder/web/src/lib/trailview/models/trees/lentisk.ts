import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Lentisk',
  latin: 'Pistacia lentiscus',
  // A dense dome on many short stems, wider than tall, leafy down to the ground.
  fork: 0.06,
  crown: [0.42, 0.75, 0.68, 0.46],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.42 + 0.58 * t)) ** 0.45,
  clumps: 16,
  billows: 6,
  stems: 3,
  leaders: true,
  heads: { count: 500, width: 0.025, height: 0.5 },
  trunk: 0.02,
  flare: 1.2,
  bark: ['#38332e', '#5a524a'],
  // Evergreen and near black, darker than holm oak and garrigue.
  leaves: {
    winter: ['#121c14', '#1e2c1a', '#3a4c2e'],
    spring: ['#131e15', '#20301c', '#425634'],
    summer: ['#121d14', '#1f2e1b', '#3e5230'],
    autumn: ['#121c14', '#1e2d1a', '#3c4e2f'],
  },
  twigs: ['#38332e', '#4a433c', '#5a524a'],
  // Tiny berries, red in late summer and black in autumn.
  flecks: { summer: ['#3e1c18', 0.03], autumn: ['#201416', 0.08] },
} satisfies Tree;
