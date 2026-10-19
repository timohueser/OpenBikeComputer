import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Phoenician juniper',
  latin: 'Juniperus phoenicea',
  base: 0.02,
  radius: 0.48,
  // A dense, rounded bush, broad from the ground up.
  shape: (t) => (1 - t ** 1.8) ** 0.6,
  overlap: 2,
  droop: 0.06,
  lobes: 7,
  jitter: 0.25,
  trunk: 0.03,
  flare: 1.3,
  bark: ['#4a3628', '#74563e'],
  // Scale leaves: yellower than prickly juniper, bronzing a little in winter.
  needles: {
    winter: ['#323a22', '#566236', '#949c5a'],
    spring: ['#2c3c22', '#526a36', '#98ae60'],
    summer: ['#2c3a22', '#506634', '#92a85c'],
    autumn: ['#2c3a22', '#506434', '#90a45a'],
  },
} satisfies Tree;
