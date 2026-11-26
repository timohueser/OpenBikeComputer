import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Almond',
  latin: 'Prunus dulcis',
  // A short trunk under an open, spreading, ragged crown.
  fork: 0.22,
  crown: [0.6, 0.5, 0.46, 0.36],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.3 + 0.7 * t)) ** 0.6,
  clumps: 13,
  open: true,
  dense: 0.7,
  billows: 4,
  bow: 0.35,
  trunk: 0.035,
  flare: 1.3,
  bark: ['#2a2826', '#46423e'],
  leaves: {
    // White to pale pink blossom on the bare branches in late winter.
    winter: ['#8e6e72', '#eed0d6', '#fdf0f2'],
    spring: ['#3c5432', '#5a7648', '#8aa270'],
    summer: ['#3c4c34', '#5e7050', '#8e9e7a'],
    autumn: ['#5e5020', '#8a7430', '#b49a4a'],
  },
  twigs: ['#2a2826', '#3a3632', '#4a4642'],
} satisfies Tree;
