import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Narrow-leaved ash',
  latin: 'Fraxinus angustifolia',
  // A tall, oval crown on a straight grey bole.
  fork: 0.34,
  crown: [0.64, 0.34, 0.32, 0.4],
  trunk: 0.04,
  flare: 1.4,
  bark: ['#56544c', '#86847a'],
  // Fine, glossy leaflets: brighter than common ash, and out weeks earlier.
  leaves: {
    winter: 'bare',
    spring: ['#4a7a2e', '#74a644', '#aed276'],
    summer: ['#2e5228', '#4a7a3a', '#7eaa5e'],
    autumn: ['#7a6a1e', '#aa9630', '#d6c056'],
  },
  twigs: ['#5a564c', '#7a766a', '#96928a'],
} satisfies Tree;
