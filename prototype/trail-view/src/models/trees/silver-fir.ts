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
    winter: ['#203d33', '#2f5644', '#4d7553'],
    spring: ['#264a3d', '#3a6e50', '#8fbf68'],
    summer: ['#244639', '#386a4e', '#76a25e'],
    autumn: ['#23443a', '#36664c', '#6c9658'],
  },
  under: '#86a597',
} satisfies Tree;
