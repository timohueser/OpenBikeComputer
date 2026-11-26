import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Gorse',
  latin: 'Ulex europaeus',
  fork: 0,
  crown: [0.5, 0.85, 0.75, 0.5],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.42 + 0.58 * t)) ** 0.5,
  clumps: 18,
  billows: 7,
  heads: { count: 150, width: 0.05, height: 0.6 },
  trunk: 0,
  flare: 1,
  bark: ['#4a3d36', '#6a5a50'],
  leaves: {
    winter: ['#28391f', '#3b5229', '#5c7239'],
    spring: ['#2e4424', '#435e2e', '#6a8042'],
    summer: ['#2e4424', '#435e2e', '#6a8042'],
    autumn: ['#2c4022', '#40592c', '#647a40'],
  },
  twigs: ['#4c3f3a', '#665650', '#80706a'],
  // Gorse flowers a little in every month and floods yellow in spring.
  flecks: { winter: ['#f2c230', 0.25], spring: ['#f6c62c', 0.6], summer: ['#f2c230', 0.2], autumn: ['#f2c230', 0.12] },
} satisfies Tree;
