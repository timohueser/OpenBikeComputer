import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Pedunculate oak',
  latin: 'Quercus robur',
  // A short, stout bole under a broad, low crown.
  fork: 0.28,
  crown: [0.6, 0.56, 0.48, 0.4],
  trunk: 0.065,
  flare: 1.5,
  neck: 0.55,
  bark: ['#4a433c', '#6c6358'],
  leaves: {
    winter: 'bare',
    // Oak leafs late, with a yellow-olive flush.
    spring: ['#55692c', '#7f9a3c', '#b8c86c'],
    summer: ['#2b4522', '#41632e', '#68884a'],
    autumn: ['#4e4a24', '#7a6e30', '#a8964a'],
  },
  twigs: ['#4a433c', '#625a52', '#7c746a'],
} satisfies Tree;
