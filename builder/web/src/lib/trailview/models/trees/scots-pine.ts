import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Scots pine',
  latin: 'Pinus sylvestris',
  fork: 0.5,
  crown: [0.78, 0.36, 0.3, 0.25],
  open: true,
  stagger: true,
  pads: true,
  sweep: 0.1,
  trunk: 0.025,
  flare: 1.5,
  // Grey plates on the lower trunk, the fox-orange of the upper trunk and limbs.
  bark: ['#4e3e34', '#c0663a'],
  fade: [0.25, 0.45],
  leaves: {
    winter: ['#314335', '#50664d', '#849978'],
    spring: ['#344736', '#576e50', '#9cb386'],
    summer: ['#324635', '#556c4e', '#92a77f'],
    autumn: ['#324435', '#54694d', '#8da37c'],
  },
  twigs: ['#7a4430', '#a85a38', '#c0663a'],
} satisfies Tree;
