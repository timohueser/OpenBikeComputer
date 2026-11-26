import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Hazel',
  latin: 'Corylus avellana',
  fork: 0.1,
  // Many rods lean out of one stool under a broad crown of few, soft lumps.
  crown: [0.58, 0.56, 0.54, 0.4],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.12 + 0.88 * t)) ** 0.5,
  clumps: 10,
  billows: 3,
  bow: 0.4,
  stems: 5,
  leaders: true,
  bunches: 40,
  trunk: 0.02,
  flare: 1.2,
  bark: ['#4e4036', '#7a6650'],
  leaves: {
    winter: 'bare',
    spring: ['#4d7137', '#678a45', '#91b44e'],
    summer: ['#2e4720', '#46662a', '#6f883d'],
    autumn: ['#7a6a26', '#a68e32', '#d2b44e'],
  },
  twigs: ['#5a4a3c', '#7a6652', '#968068'],
  // Catkins hang yellow from the bare rods in late winter.
  flecks: { winter: ['#d6bc58', 0.4] },
} satisfies Tree;
