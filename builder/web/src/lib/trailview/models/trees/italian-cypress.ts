import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Italian cypress',
  latin: 'Cupressus sempervirens',
  base: 0.04,
  radius: 0.065,
  // A dense flame: widest a third of the way up, drawn out to a point.
  shape: (t) => 1.75 * (t + 0.03) ** 0.3 * (1 - t) ** 0.85,
  column: true,
  overlap: 3,
  droop: 0.05,
  lobes: 5,
  jitter: 0.2,
  trunk: 0.02,
  flare: 1.4,
  bark: ['#4a4036', '#6e6254'],
  needles: {
    winter: ['#17281d', '#26402d', '#4a6648'],
    spring: ['#182a1e', '#28442f', '#557452'],
    summer: ['#18291e', '#28432f', '#4e6c4c'],
    autumn: ['#17291d', '#27412e', '#4b6849'],
  },
} satisfies Tree;
