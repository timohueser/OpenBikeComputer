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
    winter: ['#294225', '#446939', '#7ea05e'],
    spring: ['#2b4728', '#49703d', '#8eb267'],
    summer: ['#2a4427', '#476c3b', '#84ab62'],
    autumn: ['#294325', '#456a3a', '#80a460'],
  },
  twigs: ['#4a3a32', '#6a4a3a', '#8a5a44'],
} satisfies Tree;
