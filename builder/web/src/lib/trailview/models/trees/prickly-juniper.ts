import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Prickly juniper',
  latin: 'Juniperus oxycedrus',
  base: 0.02,
  radius: 0.36,
  // A broad, dense cone.
  shape: (t) => (1 - t) ** 0.75,
  overlap: 3,
  droop: 0.1,
  lobes: 6,
  jitter: 0.25,
  trunk: 0.03,
  flare: 1.3,
  bark: ['#4e3a2e', '#7a5a44'],
  // Two white bands on each needle silver the bush; male cones yellow it in late winter.
  needles: {
    winter: ['#323c2a', '#5e6848', '#a6a47c'],
    spring: ['#2f3e2e', '#5a6c4e', '#a2b090'],
    summer: ['#2e3c2e', '#58694c', '#9cac8c'],
    autumn: ['#2e3b2d', '#57674b', '#9aa88a'],
  },
} satisfies Tree;
