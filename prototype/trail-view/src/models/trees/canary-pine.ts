import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Canary Island pine',
  latin: 'Pinus canariensis',
  base: 0.45,
  radius: 0.21,
  shape: (t) => (1 - t) ** 0.6,
  overlap: 0.8,
  droop: 0.6,
  lobes: 4,
  jitter: 0.4,
  tufts: true,
  sprouts: 8,
  trunk: 0.024,
  flare: 1.6,
  // Old fires char the foot of the trunk.
  bark: ['#3e3430', '#86766a'],
  needles: {
    winter: ['#2c5430', '#4a8240', '#a0bf62'],
    spring: ['#2f5a32', '#4f8a42', '#b0cc68'],
    summer: ['#2e5831', '#4d8741', '#a8c862'],
    autumn: ['#2d5631', '#4b8540', '#a4c460'],
  },
} satisfies Tree;
