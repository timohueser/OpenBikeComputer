import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Common broom',
  latin: 'Cytisus scoparius',
  fork: 0.04,
  // A round sheaf of many slim green switches, taller and less dense than gorse.
  crown: [0.5, 0.5, 0.46, 0.5],
  dense: 0.55,
  rods: 24,
  heads: { count: 150, width: 0.03, height: 1.2 },
  trunk: 0.016,
  flare: 1,
  bark: ['#3e4a2a', '#56663a'],
  leaves: {
    winter: ['#1e3618', '#2e4c22', '#4c6a34'],
    spring: ['#284a1e', '#40682a', '#6c9640'],
    summer: ['#26441c', '#3c6028', '#62883e'],
    autumn: ['#22401c', '#375a26', '#5a7e3a'],
  },
  twigs: ['#2e4422', '#43602e', '#5a7a3c'],
  // Broom floods yellow in late spring, a paler gold than gorse.
  flecks: { spring: ['#fadc3a', 0.75] },
} satisfies Tree;
