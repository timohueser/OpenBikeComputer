import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Rowan',
  latin: 'Sorbus aucuparia',
  fork: 0.3,
  // A light, upswept crown of feathery leaves.
  crown: [0.62, 0.3, 0.3, 0.36],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.15 + 0.85 * t)) ** 0.7,
  clumps: 10,
  open: true,
  dense: 0.85,
  stagger: true,
  billows: 4,
  bow: 0.3,
  heads: { count: 110, width: 0.04, height: 0.5 },
  stems: 2,
  leaders: true,
  trunk: 0.022,
  flare: 1.4,
  // Smooth silver-grey bark.
  bark: ['#55524c', '#8e8b82'],
  leaves: {
    winter: 'bare',
    spring: ['#456e2c', '#6c9c40', '#a6cc68'],
    summer: ['#36521e', '#53742d', '#859a4b'],
    autumn: ['#8a3a1a', '#c4602a', '#e89a40'],
  },
  twigs: ['#5a524c', '#7a726a', '#948c82'],
  // Cream flower heads in late spring, scarlet berries from late summer.
  flecks: { spring: ['#efe8d2', 0.4], summer: ['#d8441c', 0.15], autumn: ['#c82418', 0.45] },
} satisfies Tree;
