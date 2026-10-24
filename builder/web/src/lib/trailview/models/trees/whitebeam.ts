import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Whitebeam',
  latin: 'Sorbus aria',
  // A compact, upright oval crown on a short trunk.
  fork: 0.32,
  crown: [0.6, 0.22, 0.22, 0.34],
  trunk: 0.035,
  flare: 1.4,
  // Smooth grey bark.
  bark: ['#56544e', '#86837a'],
  leaves: {
    winter: 'bare',
    // Leaves white-felted beneath, so the crown reads grey-green.
    spring: ['#5e7a4a', '#86a06a', '#b8c8a0'],
    summer: ['#354626', '#4f5b36', '#747852'],
    autumn: ['#7a5a26', '#a07a34', '#c49a4a'],
  },
  twigs: ['#56504a', '#726a62', '#8e867c'],
  // Creamy flower heads in late spring, scarlet berries in autumn.
  flecks: { spring: ['#eee6d0', 0.35], autumn: ['#b8281c', 0.35] },
} satisfies Tree;
