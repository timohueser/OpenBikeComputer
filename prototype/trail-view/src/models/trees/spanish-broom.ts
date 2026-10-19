import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Spanish broom',
  latin: 'Spartium junceum',
  fork: 0.06,
  // An upright sheaf of bare, rush-like green rods, taller and narrower than common broom.
  crown: [0.44, 0.34, 0.32, 0.56],
  stems: 5,
  trunk: 0.016,
  flare: 1.1,
  bark: ['#4a5a34', '#5e7040'],
  leaves: {
    winter: ['#344a26', '#506838', '#7a9058'],
    spring: ['#3a5428', '#5e7e3a', '#8cac5a'],
    summer: ['#3a5228', '#5c7a3a', '#88a658'],
    autumn: ['#364c26', '#546e38', '#7e9a56'],
  },
  twigs: ['#3e5030', '#56683c', '#6e824e'],
  // Large golden flowers from late spring to midsummer, later than gorse and broom.
  flecks: { spring: ['#f4cc1c', 0.2], summer: ['#f6d020', 0.35] },
} satisfies Tree;
