import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Swiss stone pine',
  latin: 'Pinus cembra',
  // A dense, broad column, leafy to the ground, with the rounded and often split top of an old tree.
  base: 0.04,
  radius: 0.24,
  shape: (t) => (1 - t ** 2.2) ** 0.6,
  overlap: 2,
  // Branch tips turn up, as a pine's do.
  droop: -0.05,
  lobes: 6,
  jitter: 0.25,
  nod: 0.04,
  trunk: 0.03,
  flare: 1.5,
  bark: ['#4a4440', '#6e6660'],
  // White lines on the needles give the dark green a blue cast.
  needles: {
    winter: ['#1c2d26', '#2f473a', '#5e7864'],
    spring: ['#1d2f27', '#324c3d', '#6a866a'],
    summer: ['#1c2e26', '#30493b', '#647e66'],
    autumn: ['#1c2e26', '#2f483a', '#607a64'],
  },
} satisfies Tree;
