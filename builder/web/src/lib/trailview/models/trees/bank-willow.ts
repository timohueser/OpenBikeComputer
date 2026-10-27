import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Bank willow',
  latin: 'Salix eleagnos',
  // Many upright rods from one stool, leafy to the ground, taller than wide.
  fork: 0.1,
  crown: [0.44, 0.34, 0.32, 0.5],
  stems: 6,
  trunk: 0.02,
  flare: 1.2,
  bark: ['#4e463c', '#72685a'],
  // Narrow leaves, white-felted beneath, grey the crown.
  leaves: {
    winter: 'bare',
    spring: ['#4e5e3c', '#7a8c60', '#b4c294'],
    summer: ['#3e4c38', '#6a7a60', '#a6b292'],
    autumn: ['#6a6a30', '#9a9444', '#c8bc64'],
  },
  // Yellow-brown to red-brown rods.
  twigs: ['#5a4632', '#7a6040', '#967a52'],
  // Slender catkins come out with the leaves.
  flecks: { spring: ['#d4cc84', 0.2] },
} satisfies Tree;
