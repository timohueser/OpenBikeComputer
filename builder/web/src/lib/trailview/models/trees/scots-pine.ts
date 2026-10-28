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
    winter: ['#27362a', '#41533e', '#6c7e62'],
    spring: ['#29392b', '#475a41', '#80946e'],
    summer: ['#28382a', '#45583f', '#788a68'],
    autumn: ['#28372a', '#44563e', '#748666'],
  },
  twigs: ['#7a4430', '#a85a38', '#c0663a'],
} satisfies Tree;
