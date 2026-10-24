import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Common juniper',
  latin: 'Juniperus communis',
  base: 0.03,
  radius: 0.26,
  // Narrow at the foot where its stems gather, widest a third of the way up, ragged to its point.
  shape: (t) => (0.65 + 1.5 * t) * (1 - t) ** 1.1,
  overlap: 3,
  droop: 0.05,
  lobes: 7,
  jitter: 0.35,
  nod: 0.05,
  trunk: 0.03,
  flare: 1.3,
  bark: ['#4a3a30', '#6a5444'],
  needles: {
    // Juniper bronzes in hard frost.
    winter: ['#2c3426', '#4a5238', '#7a7c5a'],
    spring: ['#26382c', '#3f5a44', '#7a9a70'],
    summer: ['#24362a', '#3c5642', '#70906a'],
    autumn: ['#25352a', '#3d5440', '#6c8a64'],
  },
} satisfies Tree;
