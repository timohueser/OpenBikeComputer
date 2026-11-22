import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Common ash',
  latin: 'Fraxinus excelsior',
  // An open, domed crown high on a long, pale grey bole; the limbs leave level and turn up.
  fork: 0.42,
  crown: [0.68, 0.4, 0.36, 0.32],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.1 + 0.8 * t)) ** 0.6,
  clumps: 12,
  open: true,
  dense: 0.85,
  billows: 3,
  weep: 0.25,
  bow: -0.5,
  trunk: 0.04,
  flare: 1.4,
  bark: ['#66665e', '#9a9a90'],
  // Feathery leaves let the sky through, so the crown is paler than an oak's.
  leaves: {
    winter: 'bare',
    // Ash leafs last, and drops its leaves green or pale yellow.
    spring: ['#5c6545', '#757f58', '#96a26c'],
    summer: ['#30472d', '#4e683b', '#7e905f'],
    autumn: ['#6a7034', '#9a9a48', '#c8c070'],
  },
  twigs: ['#5e5c56', '#807e76', '#a09e94'],
} satisfies Tree;
