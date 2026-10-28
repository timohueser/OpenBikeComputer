import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Black pine',
  latin: 'Pinus nigra',
  fork: 0.4,
  // Old trees spread into a broad, flat-topped crown of dense pads.
  crown: [0.72, 0.44, 0.4, 0.3],
  open: true,
  stagger: true,
  pads: true,
  trunk: 0.03,
  flare: 1.5,
  // Grey plates in dark furrows all the way up, never the orange of a Scots pine.
  bark: ['#3a3430', '#6e665e'],
  fade: [0.1, 0.6],
  leaves: {
    winter: ['#294434', '#436b4c', '#7dab75'],
    spring: ['#2c4937', '#477250', '#95cf87'],
    summer: ['#2b4735', '#466f4f', '#89bd7e'],
    autumn: ['#2b4635', '#446e4d', '#86b77b'],
  },
  twigs: ['#3a3430', '#55504a', '#6e665e'],
} satisfies Tree;
