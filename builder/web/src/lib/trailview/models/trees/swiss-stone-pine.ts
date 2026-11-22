import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Swiss stone pine',
  latin: 'Pinus cembra',
  // A dense, broad column of needle pads in whorls on a stout trunk, leafy nearly to the ground, with a round top.
  fork: 0.1,
  crown: [0.56, 0.21, 0.21, 0.42],
  open: true,
  stagger: true,
  pads: true,
  layers: 4,
  dense: 2.1,
  billows: 5,
  crook: 0.01,
  trunk: 0.03,
  flare: 1.5,
  bark: ['#4a4440', '#6e6660'],
  // White lines on the needles give the dark green a blue cast.
  leaves: {
    winter: ['#1c2d26', '#2f473a', '#5e7864'],
    spring: ['#1d2f27', '#324c3d', '#6a866a'],
    summer: ['#1c2e26', '#30493b', '#647e66'],
    autumn: ['#1c2e26', '#2f483a', '#607a64'],
  },
  twigs: ['#4a4440', '#5c5550', '#6e6660'],
} satisfies Tree;
