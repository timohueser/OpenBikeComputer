import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Silver birch',
  latin: 'Betula pendula',
  fork: 0.34,
  crown: [0.64, 0.27, 0.27, 0.34],
  open: true,
  stagger: true,
  weep: 0.6,
  trunk: 0.015,
  flare: 1.6,
  bark: ['#3a3430', '#eeebe4'],
  fade: [0.02, 0.12],
  marks: ['#1e1a18', 0.45],
  leaves: {
    winter: 'bare',
    spring: ['#5a8a32', '#8ec04c', '#cfe886'],
    summer: ['#4a7029', '#76a03c', '#b6d266'],
    autumn: ['#9a7418', '#d4a62a', '#f2d45a'],
  },
  twigs: ['#4a3436', '#6a4e4c', '#866a64'],
} satisfies Tree;
