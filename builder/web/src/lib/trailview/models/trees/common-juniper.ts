import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Common juniper',
  latin: 'Juniperus communis',
  base: 0.03,
  radius: 0.12,
  // A narrow column of needle clumps on a few stems, a little wider above its foot, with round tops.
  shape: (t) => (0.75 + 0.5 * t) * (1 - t ** 4) ** 0.5,
  column: true,
  spires: 2,
  knobs: true,
  berries: ['#2e3654', 8],
  overlap: 3,
  droop: 0.05,
  lobes: 7,
  jitter: 0.3,
  trunk: 0.03,
  flare: 1.3,
  bark: ['#4a3a30', '#6a5444'],
  needles: {
    // Grey-blue needles; juniper bronzes in hard frost.
    winter: ['#343c30', '#565e4c', '#8a8c74'],
    spring: ['#2f4036', '#546b58', '#96ac94'],
    summer: ['#2e3e35', '#516756', '#8ea48e'],
    autumn: ['#2e3d34', '#506554', '#8a9e88'],
  },
} satisfies Tree;
