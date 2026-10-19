import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Sitka spruce',
  latin: 'Picea sitchensis',
  // A plantation tree: dead lower branches, then a narrow cone of level, not hanging, branches.
  base: 0.16,
  radius: 0.16,
  shape: (t) => (1 - t) ** 0.9,
  overlap: 1.7,
  droop: 0.08,
  lobes: 7,
  jitter: 0.12,
  trunk: 0.02,
  flare: 1.7,
  bark: ['#4a3a30', '#6e5a48'],
  // White bands under the needles turn the crown blue-grey.
  under: '#8ea4a8',
  needles: {
    winter: ['#1c3034', '#2e4e50', '#587874'],
    spring: ['#1e3436', '#325656', '#90b47a'],
    summer: ['#1e3436', '#325454', '#6a8a80'],
    autumn: ['#1e3236', '#305252', '#64847a'],
  },
} satisfies Tree;
