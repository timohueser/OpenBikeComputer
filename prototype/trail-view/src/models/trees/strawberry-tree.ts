import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Strawberry tree',
  latin: 'Arbutus unedo',
  // A dense evergreen dome on a few crooked stems, leafy low down.
  fork: 0.18,
  crown: [0.52, 0.46, 0.44, 0.44],
  stems: 3,
  trunk: 0.03,
  flare: 1.3,
  // Red-brown bark that peels in strips.
  bark: ['#4e2a22', '#94482e'],
  fade: [0.05, 0.35],
  // Glossy: deep shade, bright lit tops.
  leaves: {
    winter: ['#1a3420', '#28482a', '#5e8c4a'],
    spring: ['#1c3820', '#2c4e2c', '#6e9e52'],
    summer: ['#1b3620', '#2a4c2b', '#64944e'],
    autumn: ['#1a3420', '#28482a', '#5e8c4a'],
  },
  twigs: ['#4e2a22', '#6e3a2a', '#94482e'],
  // Red fruit ripens in autumn among the white flowers, and hangs into winter.
  flecks: { autumn: ['#d8361c', 0.15], winter: ['#d84a1e', 0.08] },
} satisfies Tree;
