import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Olive',
  latin: 'Olea europaea',
  // One short, gnarled bole that twists, under a wide, low, open crown.
  fork: 0.3,
  crown: [0.64, 0.56, 0.52, 0.34],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.3 + 0.7 * t)) ** 0.5,
  clumps: 12,
  open: true,
  dense: 0.75,
  billows: 5,
  bow: 0.25,
  crook: 0.04,
  flutes: 5,
  twist: 0.5,
  neck: 0.75,
  trunk: 0.065,
  flare: 1.6,
  bark: ['#5e5a52', '#8a857a'],
  // Evergreen, silver-grey from the pale leaf undersides.
  leaves: {
    winter: ['#3c4838', '#62705a', '#9eaa8e'],
    spring: ['#3e4a38', '#66745a', '#a4ae90'],
    summer: ['#3e4a38', '#66745a', '#a4ae90'],
    autumn: ['#3c4838', '#64725a', '#a0ac8e'],
  },
  twigs: ['#5a564e', '#77726a', '#948e84'],
} satisfies Tree;
