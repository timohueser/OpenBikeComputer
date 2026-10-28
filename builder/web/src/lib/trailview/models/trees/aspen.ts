import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Aspen',
  latin: 'Populus tremula',
  // A slender, straight stem under a small, rounded, open crown high up.
  fork: 0.45,
  crown: [0.72, 0.24, 0.23, 0.26],
  open: true,
  stagger: true,
  trunk: 0.016,
  flare: 1.4,
  // Smooth grey-green bark, darker and cracked at the foot.
  bark: ['#4e4c44', '#a8ac98'],
  fade: [0.03, 0.15],
  marks: ['#4a4840', 0.15],
  // Round leaves on flat stalks flutter and show their pale undersides.
  leaves: {
    winter: 'bare',
    spring: ['#4a6a30', '#6e9442', '#a8c472'],
    summer: ['#3a5630', '#587a44', '#8aa874'],
    autumn: ['#a8700e', '#e0a020', '#f4c840'],
  },
  twigs: ['#4e4a42', '#6a665c', '#86827a'],
  // Grey woolly catkins hang from the bare twigs in early spring.
  flecks: { winter: ['#9a968a', 0.2] },
} satisfies Tree;
