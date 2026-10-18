import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Olive',
  latin: 'Olea europaea',
  // Limbs lean out from one gnarled foot under a wide, low crown.
  fork: 0.26,
  crown: [0.6, 0.56, 0.52, 0.38],
  stems: 3,
  trunk: 0.05,
  flare: 1.8,
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
