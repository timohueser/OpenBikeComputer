import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Black alder',
  latin: 'Alnus glutinosa',
  // A straight stem up a narrow, near conical crown of very dark leaves.
  fork: 0.25,
  crown: [0.6, 0.22, 0.22, 0.46],
  trunk: 0.03,
  flare: 1.5,
  bark: ['#3a3430', '#5a524a'],
  leaves: {
    winter: 'bare',
    spring: ['#3a5e28', '#5a8436', '#8cb060'],
    summer: ['#1e3a1c', '#2e5228', '#4e7440'],
    // Alder drops its leaves green or brown, without autumn colour.
    autumn: ['#2e3e1e', '#4a5a2c', '#6e7640'],
  },
  // Purple-brown twigs and catkins.
  twigs: ['#3e3236', '#5a4a4c', '#76646a'],
} satisfies Tree;
