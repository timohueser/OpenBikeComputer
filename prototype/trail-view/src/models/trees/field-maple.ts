import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Field maple',
  latin: 'Acer campestre',
  // A small, dense, round crown low on a short bole, often in a hedge.
  fork: 0.2,
  crown: [0.54, 0.46, 0.44, 0.42],
  trunk: 0.045,
  flare: 1.4,
  // Grey-brown bark in corky ridges.
  bark: ['#4a423a', '#766c60'],
  leaves: {
    winter: 'bare',
    spring: ['#5e7e2c', '#8aac42', '#c2d878'],
    summer: ['#2e5026', '#467234', '#6c964a'],
    // Clear butter yellow, brighter than sycamore.
    autumn: ['#9e7614', '#d0a826', '#f0d652'],
  },
  twigs: ['#56493e', '#726458', '#8e8072'],
} satisfies Tree;
