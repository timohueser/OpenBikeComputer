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
    winter: ['#37585e', '#55898c', '#9acec8'],
    spring: ['#3a5e62', '#5b9696', '#cdffaf'],
    summer: ['#3a5e62', '#5b9393', '#b7ecdb'],
    autumn: ['#3a5b62', '#589090', '#ade2d1'],
  },
} satisfies Tree;
