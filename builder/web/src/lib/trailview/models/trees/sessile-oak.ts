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
    spring: ['#576a36', '#748745', '#a1b254'],
    summer: ['#314428', '#435b34', '#627548'],
    autumn: ['#6a4e22', '#9a702e', '#c49646'],
  },
  twigs: ['#4e4842', '#665e56', '#80786e'],
} satisfies Tree;
