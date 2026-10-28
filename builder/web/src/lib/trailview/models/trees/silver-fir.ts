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
    winter: ['#325145', '#4b735a', '#7ca77c'],
    spring: ['#395d4b', '#548460', '#c8ff8e'],
    summer: ['#355646', '#517a5d', '#8bb97f'],
    autumn: ['#355445', '#50795a', '#88b67c'],
  },
  under: '#7e9488',
} satisfies Tree;
