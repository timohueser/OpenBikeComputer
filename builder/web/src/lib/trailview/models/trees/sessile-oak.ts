import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Sessile oak',
  latin: 'Quercus petraea',
  // A long, straight bole under a higher, more regular crown than the pedunculate oak's; a slight
  // lean keeps it from reading as a beech.
  fork: 0.44,
  crown: [0.69, 0.4, 0.34, 0.32],
  sweep: 0.06,
  trunk: 0.045,
  flare: 1.4,
  bark: ['#4e4842', '#736a60'],
  leaves: {
    winter: 'bare',
    spring: ['#5a722e', '#86a240', '#bed070'],
    summer: ['#2f4a22', '#486e30', '#7a9a4c'],
    autumn: ['#6a4e22', '#9a702e', '#c49646'],
  },
  twigs: ['#4e4842', '#665e56', '#80786e'],
} satisfies Tree;
