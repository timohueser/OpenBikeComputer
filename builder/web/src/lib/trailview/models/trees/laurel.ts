import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Laurel forest tree',
  latin: 'Laurus novocanariensis',
  fork: 0.38,
  // Taller and narrower than the holm oak.
  crown: [0.6, 0.32, 0.32, 0.42],
  shape: t => Math.sin(Math.PI * Math.min(1, 0.12 + 0.88 * t)) ** 0.6,
  clumps: 12,
  billows: 3,
  bow: 0.3,
  stems: 3,
  leaders: true,
  trunk: 0.026,
  flare: 1.5,
  // Moss climbs the trunks in the cloud forest.
  bark: ['#4a5236', '#6e675c'],
  // Glossy: deep shade, bright lit tops.
  leaves: {
    winter: ['#1a3a26', '#1f452b', '#5a964a'],
    spring: ['#1b3c27', '#22492d', '#6aa452'],
    summer: ['#1a3b26', '#21472c', '#5e9c4c'],
    autumn: ['#1a3a26', '#1f452b', '#5a964a'],
  },
  twigs: ['#4a453c', '#6e675c', '#8a8276'],
} satisfies Tree;
