import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Turkey oak',
  latin: 'Quercus cerris',
  // A tall, straight bole under an upright oval crown.
  fork: 0.36,
  crown: [0.62, 0.3, 0.28, 0.4],
  trunk: 0.045,
  flare: 1.4,
  bark: ['#36302c', '#4e4640'],
  leaves: {
    winter: 'bare',
    spring: ['#4e6630', '#7a9446', '#aec27a'],
    summer: ['#273f22', '#3c5a30', '#5e7e4e'],
    autumn: ['#5e4a24', '#86682e', '#a88a46'],
  },
  twigs: ['#3e3834', '#58504a', '#726a62'],
} satisfies Tree;
