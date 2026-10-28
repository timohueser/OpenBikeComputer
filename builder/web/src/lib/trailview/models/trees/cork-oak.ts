import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Cork oak',
  latin: 'Quercus suber',
  // A dehesa tree: a short, thick bole and crooked limbs under a broad, rounded, open crown.
  fork: 0.24,
  crown: [0.6, 0.66, 0.62, 0.42],
  open: true,
  trunk: 0.06,
  flare: 1.5,
  // The lower bole stripped of its cork to red-orange, the limbs above in thick grey cork.
  bark: ['#b4542c', '#6e665c'],
  fade: [0.2, 0.26],
  // Evergreen: dark leaves with grey felted undersides.
  leaves: {
    winter: ['#25301f', '#3c4a30', '#64724e'],
    spring: ['#28351f', '#42532f', '#73865a'],
    summer: ['#25311e', '#3d4c2f', '#67784e'],
    autumn: ['#25301f', '#3c4a30', '#64744e'],
  },
  twigs: ['#5a5248', '#6e665c', '#8a8276'],
} satisfies Tree;
