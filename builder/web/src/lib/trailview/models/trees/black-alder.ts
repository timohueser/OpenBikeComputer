import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Black alder',
  latin: 'Alnus glutinosa',
  // A straight stem up through a cone of very dark leaves, on level limbs.
  fork: 0.25,
  crown: [0.6, 0.25, 0.25, 0.44],
  shape: t => 1 - 0.85 * Math.max(0, t - 0.15),
  clumps: 15,
  stagger: true,
  billows: 3,
  bow: -0.25,
  trunk: 0.03,
  flare: 1.5,
  bark: ['#3a3430', '#5a524a'],
  leaves: {
    winter: 'bare',
    spring: ['#3a5e28', '#5a8436', '#8cb060'],
    summer: ['#1b3014', '#2a441e', '#486132'],
    // Alder drops its leaves green or brown, without autumn colour.
    autumn: ['#2e3e1e', '#4a5a2c', '#6e7640'],
  },
  // Purple-brown twigs and catkins.
  twigs: ['#3e3236', '#5a4a4c', '#76646a'],
} satisfies Tree;
