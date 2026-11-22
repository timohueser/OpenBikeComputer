import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Phoenician juniper',
  latin: 'Juniperus phoenicea',
  // A dense, rounded bush on a short, stout stem, its outline in knobs.
  fork: 0.12,
  crown: [0.54, 0.47, 0.45, 0.46],
  billows: 6,
  sweep: 0.12,
  berries: ['#6e3524', 6],
  trunk: 0.04,
  flare: 1.3,
  bark: ['#4a3628', '#74563e'],
  // Scale leaves: yellower than prickly juniper, bronzing a little in winter.
  leaves: {
    winter: ['#323a22', '#566236', '#949c5a'],
    spring: ['#2c3c22', '#526a36', '#98ae60'],
    summer: ['#2c3a22', '#506634', '#92a85c'],
    autumn: ['#2c3a22', '#506434', '#90a45a'],
  },
  twigs: ['#4a3628', '#5e4632', '#74563e'],
} satisfies Tree;
