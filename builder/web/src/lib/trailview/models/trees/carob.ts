import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Carob',
  latin: 'Ceratonia siliqua',
  // A short, thick trunk under a broad, dense dome.
  fork: 0.2,
  crown: [0.6, 0.7, 0.64, 0.4],
  trunk: 0.06,
  flare: 1.6,
  bark: ['#4a4038', '#6e6458'],
  // Evergreen and glossy: deep shade, bright lit tops.
  leaves: {
    winter: ['#15291a', '#223e28', '#4c7444'],
    spring: ['#162b1a', '#24422a', '#547e4a'],
    summer: ['#152a1a', '#234029', '#507a46'],
    autumn: ['#15291a', '#223e28', '#4e7644'],
  },
  twigs: ['#4a4038', '#5c5248', '#6e6458'],
  // Long brown pods hang under the leaves from late summer.
  flecks: { summer: ['#54381e', 0.05], autumn: ['#3e2816', 0.1] },
} satisfies Tree;
