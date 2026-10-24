import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Stone pine',
  latin: 'Pinus pinea',
  fork: 0.64,
  // The umbrella: a dense, flat-topped crown on limbs that fan out from the top of a long bole.
  crown: [0.8, 0.38, 0.38, 0.2],
  pads: true,
  trunk: 0.032,
  flare: 1.4,
  // Grey-brown below, orange-red plates higher up.
  bark: ['#4a3a32', '#8a5a44'],
  fade: [0.1, 0.7],
  leaves: {
    winter: ['#22381f', '#3a5a30', '#6c8a50'],
    spring: ['#243c21', '#3e6034', '#7a9a58'],
    summer: ['#233a20', '#3c5d32', '#729454'],
    autumn: ['#22391f', '#3b5b31', '#6e8e52'],
  },
  twigs: ['#4a3a32', '#6a4a3a', '#8a5a44'],
} satisfies Tree;
