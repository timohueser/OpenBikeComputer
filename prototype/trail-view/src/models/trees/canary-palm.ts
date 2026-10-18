import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Canary Island date palm',
  latin: 'Phoenix canariensis',
  fork: 0.66,
  crown: [0.7, 0.36, 0.36, 0.3],
  fronds: 48,
  trunk: 0.045,
  neck: 1,
  flare: 1.3,
  bark: ['#5e5040', '#8a7860'],
  scars: 7,
  leaves: {
    winter: ['#2e4a26', '#557a34', '#a4bc5e'],
    spring: ['#2f4c27', '#587e36', '#a8c062'],
    summer: ['#2e4a26', '#557a34', '#a4bc5e'],
    autumn: ['#2e4a26', '#557a34', '#a4bc5e'],
  },
  twigs: ['#5e5040', '#7a6a54', '#8a7860'],
} satisfies Tree;
