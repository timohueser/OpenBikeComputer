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
    winter: ['#223b37', '#3a6258', '#6f9283'],
    spring: ['#25423c', '#406c5f', '#86a893'],
    summer: ['#24413b', '#3f6a5e', '#7d9f8c'],
    autumn: ['#24403a', '#3e685c', '#7a9a88'],
  },
  twigs: ['#7a4430', '#a85a38', '#c0663a'],
} satisfies Tree;
