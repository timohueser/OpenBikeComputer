import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'White willow',
  latin: 'Salix alba',
  // Two leaning stems from one foot under a broad, billowing crown whose shoots hang a little.
  fork: 0.28,
  crown: [0.62, 0.48, 0.44, 0.38],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.2 + 0.75 * t)) ** 0.5,
  clumps: 12,
  open: true,
  dense: 1,
  billows: 4,
  weep: 0.35,
  bow: 0.4,
  sweep: 0.06,
  stems: 2,
  leaders: true,
  trunk: 0.04,
  flare: 1.5,
  // Grey bark in deep criss-cross ridges.
  bark: ['#4a443c', '#7a7266'],
  // Silky white leaf undersides silver the crown.
  leaves: {
    winter: 'bare',
    spring: ['#56683c', '#889e60', '#c2d098'],
    summer: ['#46553c', '#74866a', '#b0bca0'],
    autumn: ['#6a6634', '#968f50', '#c6be7a'],
  },
  // Yellow-olive shoots give the bare crown a golden haze.
  twigs: ['#6a5a3a', '#8c7a46', '#ac9a5a'],
} satisfies Tree;
