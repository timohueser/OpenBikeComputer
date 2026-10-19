import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Downy birch',
  latin: 'Betula pubescens',
  fork: 0.3,
  // Often on two stems, with an irregular crown of upswept, not hanging, twigs.
  crown: [0.62, 0.32, 0.3, 0.36],
  open: true,
  stagger: true,
  stems: 2,
  trunk: 0.016,
  flare: 1.4,
  // Smooth grey-white bark with few dark marks, and no black, cracked foot.
  bark: ['#7a726a', '#e2ded6'],
  fade: [0, 0.08],
  marks: ['#4a4440', 0.2],
  leaves: {
    winter: 'bare',
    spring: ['#4e7a2c', '#7aaa42', '#b6d874'],
    // Duller and darker than silver birch.
    summer: ['#3a5a26', '#5a8034', '#8eae56'],
    autumn: ['#94661a', '#c6922a', '#e8c04e'],
  },
  // Purple-brown twigs give the bare crown a purple haze.
  twigs: ['#4a3234', '#664a4a', '#806664'],
} satisfies Tree;
