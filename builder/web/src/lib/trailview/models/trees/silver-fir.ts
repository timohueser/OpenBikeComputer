import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Silver fir',
  latin: 'Abies alba',
  base: 0.1,
  radius: 0.2,
  // Columnar, with the blunt top of an older fir.
  shape: (t) => 1 - t ** 2.2,
  overlap: 1.6,
  droop: 0.06,
  lobes: 8,
  jitter: 0.06,
  trunk: 0.02,
  flare: 1.6,
  bark: ['#5a5852', '#8a877e'],
  needles: {
    winter: ['#1c3028', '#2c4636', '#4c684c'],
    spring: ['#20382c', '#32513a', '#86ac5e'],
    summer: ['#1e3329', '#304b38', '#56744e'],
    autumn: ['#1e3228', '#2f4a36', '#54724c'],
  },
  under: '#7e9488',
} satisfies Tree;
