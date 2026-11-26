import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Black pine',
  latin: 'Pinus nigra',
  fork: 0.34,
  // Old trees spread into a broad, dark dome of heavy pads in level whorls.
  crown: [0.68, 0.42, 0.4, 0.3],
  open: true,
  stagger: true,
  pads: true,
  billows: 5,
  dense: 1.3,
  layers: 3,
  crook: 0.006,
  trunk: 0.03,
  flare: 1.5,
  // Grey plates in dark furrows all the way up, never the orange of a Scots pine.
  bark: ['#3a3430', '#6e665e'],
  fade: [0.1, 0.6],
  leaves: {
    winter: ['#17291e', '#28432e', '#4f6e4a'],
    spring: ['#192c20', '#2b4831', '#5f8656'],
    summer: ['#182b1f', '#2a4630', '#577a50'],
    autumn: ['#182a1f', '#29452f', '#55764e'],
  },
  twigs: ['#3a3430', '#55504a', '#6e665e'],
} satisfies Tree;
