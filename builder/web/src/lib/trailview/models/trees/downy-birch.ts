import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Downy birch',
  latin: 'Betula pubescens',
  fork: 0.3,
  // Often on two stems, with an irregular crown of upswept, not hanging, twigs.
  crown: [0.62, 0.32, 0.3, 0.36],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.12 + 0.88 * t)) ** 0.8,
  clumps: 12,
  open: true,
  dense: 0.85,
  stagger: true,
  billows: 4,
  bow: 0.35,
  stems: 2,
  leaders: true,
  trunk: 0.016,
  flare: 1.4,
  // Smooth grey-white bark without the silver birch's black marks and cracked foot.
  bark: ['#7a726a', '#e2ded6'],
  fade: [0, 0.08],
  leaves: {
    winter: 'bare',
    spring: ['#4a682c', '#6e8e40', '#a4bc70'],
    // Duller and darker than silver birch.
    summer: ['#354b1c', '#536b28', '#839243'],
    autumn: ['#94661a', '#c6922a', '#e8c04e'],
  },
  // Purple-brown twigs give the bare crown a purple haze.
  twigs: ['#4a3234', '#664a4a', '#806664'],
} satisfies Tree;
