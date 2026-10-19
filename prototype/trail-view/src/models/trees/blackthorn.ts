import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Blackthorn',
  latin: 'Prunus spinosa',
  fork: 0.1,
  // A low, dense thicket of suckers, wider than tall.
  crown: [0.46, 0.64, 0.52, 0.4],
  stems: 5,
  trunk: 0.02,
  flare: 1.2,
  bark: ['#262020', '#3e3634'],
  leaves: {
    winter: 'bare',
    // White blossom on black twigs before the leaves.
    spring: ['#7a7470', '#d4d0c8', '#f6f4ee'],
    summer: ['#24391e', '#38542a', '#5c7a42'],
    autumn: ['#4a4426', '#6a6034', '#8e8248'],
  },
  twigs: ['#262020', '#3a3230', '#504644'],
  // Black twigs show through the blossom; blue-black sloes in autumn.
  flecks: { spring: ['#5a524c', 0.12], autumn: ['#2c2e4a', 0.15] },
} satisfies Tree;
