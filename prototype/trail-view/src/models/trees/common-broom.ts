import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Common broom',
  latin: 'Cytisus scoparius',
  fork: 0,
  // An upright sheaf of green switches, taller and less round than gorse.
  crown: [0.42, 0.45, 0.4, 0.58],
  trunk: 0,
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
