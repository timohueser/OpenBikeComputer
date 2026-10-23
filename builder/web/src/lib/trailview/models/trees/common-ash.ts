import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Common ash',
  latin: 'Fraxinus excelsior',
  // A domed crown high on a long, pale grey bole.
  fork: 0.42,
  crown: [0.68, 0.4, 0.36, 0.32],
  trunk: 0.04,
  flare: 1.4,
  bark: ['#66665e', '#9a9a90'],
  // Feathery leaves let the sky through, so the crown is paler than an oak's.
  leaves: {
    winter: 'bare',
    // Ash leafs last, and drops its leaves green or pale yellow.
    spring: ['#5e6a40', '#86945a', '#b4c08a'],
    summer: ['#34563a', '#547c4c', '#88ac78'],
    autumn: ['#6a7034', '#9a9a48', '#c8c070'],
  },
  twigs: ['#5e5c56', '#807e76', '#a09e94'],
} satisfies Tree;
