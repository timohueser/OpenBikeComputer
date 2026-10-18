import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Hop hornbeam',
  latin: 'Ostrya carpinifolia',
  // A small tree of coppiced limestone slopes: several stems under a rounded crown.
  fork: 0.28,
  crown: [0.58, 0.36, 0.34, 0.42],
  stems: 3,
  trunk: 0.022,
  flare: 1.3,
  bark: ['#4e4640', '#7a6e64'],
  leaves: {
    winter: 'bare',
    spring: ['#4a7a30', '#72a446', '#a8cc74'],
    summer: ['#2a4822', '#426a30', '#6a904a'],
    autumn: ['#6e6224', '#a08c30', '#ccb44e'],
  },
  twigs: ['#4e4640', '#6a6058', '#847a70'],
  // Pale hop-like fruit clusters hang all over the crown from midsummer.
  flecks: { summer: ['#dcdcb0', 0.12], autumn: ['#c8b88a', 0.1] },
} satisfies Tree;
