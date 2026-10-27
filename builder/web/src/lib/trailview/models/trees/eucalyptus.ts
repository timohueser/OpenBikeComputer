import type { Tree } from '../trees';

export default {
  form: 'broadleaf',
  name: 'Eucalyptus',
  latin: 'Eucalyptus globulus',
  // A plantation tree: a tall, straight, bare bole under a narrow, open crown of hanging leaves.
  fork: 0.5,
  crown: [0.74, 0.2, 0.18, 0.3],
  open: true,
  stagger: true,
  weep: 0.6,
  trunk: 0.014,
  flare: 1.6,
  neck: 0.5,
  // Rough grey-brown bark at the foot; above it the bark sheds in strips to a smooth cream-grey.
  bark: ['#6e6252', '#d6d0c0'],
  fade: [0.04, 0.2],
  // Evergreen: grey-green leaves with a blue cast, a little paler with the spring shoots.
  leaves: {
    winter: ['#384840', '#5e7062', '#94a294'],
    spring: ['#3a4a40', '#647666', '#a0ae98'],
    summer: ['#3a4a40', '#627464', '#9aa898'],
    autumn: ['#384840', '#5e7062', '#94a294'],
  },
  twigs: ['#6e6252', '#9a9080', '#c4bcaa'],
} satisfies Tree;
