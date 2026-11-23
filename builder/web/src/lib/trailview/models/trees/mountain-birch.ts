import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Mountain birch',
  latin: 'Betula pubescens subsp. czerepanovii',
  // A low thicket: stems lean out from one base under a broad, loose crown of small-leaved clumps.
  fork: 0.2,
  crown: [0.6, 0.52, 0.48, 0.38],
  crook: 0.035,
  shape: t => Math.sin(Math.PI * Math.min(1, 0.3 + 0.7 * t)) ** 0.6,
  clumps: 10,
  open: true,
  dense: 0.9,
  billows: 4,
  bow: 0.2,
  sweep: 0.08,
  stems: 3,
  leaders: true,
  trunk: 0.022,
  flare: 1.3,
  // Grey bark, darker at the foot; no dark marks, which on five near stems would nearly double the
  // tree's triangles.
  bark: ['#5a5650', '#a8a49c'],
  fade: [0, 0.1],
  // Small leaves: dark olive in summer, gold in autumn.
  leaves: {
    winter: 'bare',
    spring: ['#3e5226', '#5a7034', '#7e9450'],
    summer: ['#36421f', '#4a5a2c', '#6a7a3c'],
    autumn: ['#9a6a14', '#d09a24', '#f0c848'],
  },
  // Purple-brown twigs give the bare thicket a purple haze.
  twigs: ['#4a3036', '#664650', '#806068'],
} satisfies Tree;
