import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Narrow-leaved ash',
  latin: 'Fraxinus angustifolia',
  // A tall, oval crown on a straight grey bole.
  fork: 0.34,
  crown: [0.64, 0.34, 0.32, 0.4],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.1 + 0.9 * t)) ** 0.8,
  clumps: 13,
  open: true,
  dense: 0.82,
  billows: 4,
  weep: 0.45,
  bow: 0.2,
  trunk: 0.04,
  flare: 1.4,
  bark: ['#56544c', '#86847a'],
  // Fine, glossy leaflets: brighter than common ash, and out weeks earlier.
  leaves: {
    winter: 'bare',
    spring: ['#4a7a2e', '#74a644', '#aed276'],
    summer: ['#2a441e', '#45662d', '#758f4a'],
    autumn: ['#7a6a1e', '#aa9630', '#d6c056'],
  },
  twigs: ['#5a564c', '#7a766a', '#96928a'],
} satisfies Tree;
