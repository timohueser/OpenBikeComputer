import type { Tree } from '../trees';

export default {
  form: 'conifer',
  name: 'Phoenician juniper',
  latin: 'Juniperus phoenicea',
  base: 0.03,
  radius: 0.4,
  // A dense bush of scale-leaf tufts, broad from the ground up to a blunt, round top.
  shape: (t) => (1 - t ** 2.2) ** 0.6,
  column: true,
  knobs: true,
  berries: ['#6e3524', 6],
  overlap: 2,
  droop: 0.06,
  lobes: 7,
  jitter: 0.14,
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
