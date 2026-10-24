import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Maritime pine',
  latin: 'Pinus pinaster',
  fork: 0.6,
  // A small, ragged crown of needle brushes high on a long bole.
  crown: [0.82, 0.33, 0.3, 0.19],
  open: true,
  stagger: true,
  // Most lean, on sand and in the wind off the Atlantic; it takes a random heading like any other tree.
  sweep: 0.16,
  trunk: 0.028,
  flare: 1.5,
  // Thick bark: near-black furrows between purple-red plates.
  bark: ['#2e2422', '#6a4234'],
  fade: [0.2, 0.8],
  leaves: {
    winter: ['#2e4224', '#506a34', '#8aa05a'],
    spring: ['#30462a', '#567238', '#9cb466'],
    summer: ['#2f4426', '#536e36', '#94ac60'],
    autumn: ['#2e4325', '#516c35', '#8ea65c'],
  },
  twigs: ['#3a2a24', '#5a3c30', '#6a4234'],
} satisfies Tree;
