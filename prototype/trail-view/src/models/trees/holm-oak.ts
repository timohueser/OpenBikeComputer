import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Holm oak',
  latin: 'Quercus ilex',
  fork: 0.24,
  crown: [0.56, 0.46, 0.46, 0.38],
  stems: 3,
  trunk: 0.04,
  flare: 1.4,
  bark: ['#3e3a35', '#5e5850'],
  leaves: {
    winter: ['#26301e', '#3a4a2a', '#5a6a40'],
    spring: ['#29381f', '#3f542b', '#6c8646'],
    summer: ['#25331d', '#3a4e29', '#5f7740'],
    autumn: ['#26321e', '#3b4d2a', '#5e7240'],
  },
  twigs: ['#3e3a35', '#5e5850', '#7a736a'],
} satisfies Tree;
