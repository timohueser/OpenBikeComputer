import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Western hemlock',
  latin: 'Tsuga heterophylla',
  base: 0.08,
  radius: 0.16,
  shape: (t) => 1 - t,
  overlap: 1.9,
  droop: 0.24,
  lobes: 8,
  jitter: 0.1,
  nod: 0.06,
  trunk: 0.017,
  flare: 1.7,
  bark: ['#4a3024', '#6a4a3a'],
  needles: {
    winter: ['#203d30', '#2f5a42', '#5a8256'],
    spring: ['#264a3a', '#3a7050', '#9cc76e'],
    summer: ['#244736', '#386c4c', '#7cab60'],
    autumn: ['#234534', '#36684a', '#74a25c'],
  },
} satisfies Tree;
