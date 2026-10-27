import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Pyrenean oak',
  latin: 'Quercus pyrenaica',
  // Coppice stools of two leaning stems under an open, irregular crown.
  fork: 0.3,
  crown: [0.62, 0.42, 0.38, 0.36],
  sweep: 0.05,
  stems: 2,
  trunk: 0.035,
  flare: 1.4,
  bark: ['#4a423a', '#6e665a'],
  // Felted leaves: silvery as they unfold, grey-green all summer.
  leaves: {
    winter: 'bare',
    spring: ['#5e6648', '#8c9670', '#c4c8a4'],
    summer: ['#3e4a2e', '#66734e', '#9ea67e'],
    autumn: ['#6e5428', '#9a7838', '#c49e58'],
  },
  twigs: ['#4a4038', '#645a50', '#7e746a'],
  // Most dead leaves hang on, russet, until the new ones push them off in May.
  flecks: { winter: ['#9a6a3c', 0.75] },
} satisfies Tree;
