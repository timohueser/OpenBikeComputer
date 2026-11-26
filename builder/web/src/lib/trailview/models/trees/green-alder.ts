import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Green alder',
  latin: 'Alnus viridis',
  // A low, dense thicket of many stems that snow bends down the slope, wider than tall.
  fork: 0.06,
  crown: [0.46, 0.66, 0.58, 0.42],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.2 + 0.8 * t)) ** 0.5,
  clumps: 12,
  open: true,
  dense: 0.8,
  billows: 3,
  bow: 0.5,
  sweep: 0.25,
  stems: 4,
  leaders: true,
  bunches: 36,
  trunk: 0.02,
  flare: 1.2,
  bark: ['#4a4440', '#6e6660'],
  // Glossy, fresh green leaves that fall late, with little autumn colour.
  leaves: {
    winter: 'bare',
    spring: ['#3e6c2c', '#5e963e', '#94c062'],
    summer: ['#2a4a22', '#42683a', '#6c944e'],
    autumn: ['#5a5a26', '#827e38', '#aaa252'],
  },
  twigs: ['#4a4038', '#665a50', '#82766a'],
  // Yellow catkins hang out with the new leaves; dark catkins and cones wait on the bare twigs.
  flecks: { spring: ['#c8c060', 0.25], winter: ['#5a4034', 0.2] },
} satisfies Tree;
