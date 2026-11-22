import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Prickly juniper',
  latin: 'Juniperus oxycedrus',
  // A small, open tree on a few stems, its branch ends hanging.
  fork: 0.3,
  crown: [0.66, 0.36, 0.34, 0.32],
  open: true,
  stems: 3,
  leaders: true,
  weep: 0.35,
  billows: 4,
  dense: 1.3,
  crook: 0.02,
  berries: ['#7a3a26', 6],
  trunk: 0.022,
  flare: 1.3,
  bark: ['#4e3a2e', '#7a5a44'],
  // Two white bands on each needle silver the bush; male cones yellow it in late winter.
  leaves: {
    winter: ['#323c2a', '#5e6848', '#a6a47c'],
    spring: ['#2f3e2e', '#5a6c4e', '#a2b090'],
    summer: ['#2e3c2e', '#58694c', '#9cac8c'],
    autumn: ['#2e3b2d', '#57674b', '#9aa88a'],
  },
  twigs: ['#4e3a2e', '#64493a', '#7a5a44'],
} satisfies Tree;
