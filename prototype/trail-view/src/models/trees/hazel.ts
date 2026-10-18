import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Hazel',
  latin: 'Corylus avellana',
  fork: 0.15,
  // Many rods from one stool under a broad mound of soft leaves.
  crown: [0.48, 0.55, 0.55, 0.42],
  stems: 6,
  trunk: 0.025,
  flare: 1.2,
  bark: ['#4e4036', '#7a6650'],
  leaves: {
    winter: 'bare',
    spring: ['#4a7a2e', '#72a640', '#aed268'],
    summer: ['#32562a', '#4c7a36', '#78a24e'],
    autumn: ['#7a6a26', '#a68e32', '#d2b44e'],
  },
  twigs: ['#5a4a3c', '#7a6652', '#968068'],
  // Catkins hang yellow from the bare rods in late winter.
  flecks: { winter: ['#d6bc58', 0.4] },
} satisfies Tree;
