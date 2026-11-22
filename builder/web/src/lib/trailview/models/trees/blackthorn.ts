import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Blackthorn',
  latin: 'Prunus spinosa',
  fork: 0.1,
  // A low thicket of suckers, wider than tall, in many small lumps on black limbs.
  crown: [0.46, 0.64, 0.52, 0.4],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.35 + 0.65 * t)) ** 0.5,
  clumps: 15,
  open: true,
  dense: 0.75,
  billows: 5,
  bow: 0.35,
  stems: 3,
  leaders: true,
  heads: { count: 130, width: 0.03, height: 0.5 },
  trunk: 0.02,
  flare: 1.2,
  bark: ['#262020', '#3e3634'],
  leaves: {
    winter: 'bare',
    // White blossom on black twigs before the leaves.
    spring: ['#3e3634', '#a8a29a', '#f6f4ee'],
    summer: ['#212f16', '#334620', '#556633'],
    autumn: ['#4a4426', '#6a6034', '#8e8248'],
  },
  twigs: ['#262020', '#3a3230', '#504644'],
  // Blue-black sloes in autumn.
  flecks: { autumn: ['#2c2e4a', 0.15] },
} satisfies Tree;
