/**
 * Tree species, each at three levels of detail from one builder. A tree is 1 tall with its trunk
 * base at the origin and z up; the instanced shader scales it by the tree's height. The season
 * only changes vertex colours.
 */

import * as THREE from 'three';
import { hash, hex, lathe, limb, lump, merge, paint, place, random, smooth, soft, unit, v3, type Vec } from './shape';

/** The order of SEASONS in light.ts. */
export const SEASONS = ['winter', 'spring', 'summer', 'autumn'] as const;
export type Season = (typeof SEASONS)[number];

/** 0 within 40 m, 1 within 200 m, 2 out to 1 km. */
export type Lod = 0 | 1 | 2;
export const LOD_RANGE_M = [40, 200, 1000] as const;

/** Segments around a part, spline rows per tier, conifer tiers and broadleaf crown lumps. */
const DETAIL = [
  { sides: 22, rows: 10, tiers: 7, trunk: [10, 8], lumps: 9, ico: 3 },
  { sides: 10, rows: 6, tiers: 5, trunk: [6, 4], lumps: 6, ico: 1 },
  { sides: 6, rows: 3, tiers: 3, trunk: [4, 3], lumps: 4, ico: 0 },
] as const;

/** Shade, body and lit-tip colour. */
type Ramp = [string, string, string];

interface Bark {
  /** Trunk radius at the ground, and the root flare in trunk radii. */
  trunk: number;
  flare: number;
  /** Colour at the ground and higher up. */
  bark: [string, string];
}

interface Conifer extends Bark {
  form: 'conifer';
  name: string;
  /** Crown base height and crown radius there, in tree heights. */
  base: number;
  radius: number;
  /** The crown's radius from 1 at its base (t = 0) to its top (t = 1). */
  shape: (t: number) => number;
  /** Tier height in tier spacings. Near 1 the trunk shows between the tiers. */
  overlap: number;
  /** How far branch tips hang, in tier heights, and how many tips a tier has. */
  droop: number;
  lobes: number;
  /** Tier radius and position vary by this share from tier to tier. */
  jitter: number;
  needles: Record<Season, Ramp | 'bare'>;
  /** The twigs of a conifer that goes bare: a near tree shows bare branches, a far one a haze. */
  twigs?: Ramp;
  /** Pale needle undersides, as on silver fir. */
  under?: string;
  /** How far the leader nods over towards +x, in tree heights. */
  nod?: number;
}

interface Broadleaf extends Bark {
  form: 'broadleaf';
  name: string;
  /** Where the trunk divides. A bush has no trunk. */
  fork: number;
  /** Crown centre height and half-sizes along x, y and z. */
  crown: [number, number, number, number];
  /** How far the top is blown over towards +x, in tree heights. */
  sweep?: number;
  leaves: Record<Season, Ramp | 'bare'>;
  /** Bare twigs: near trees show limbs in this colour, far ones a haze of it. */
  twigs: Ramp;
  /** Blossom, berries or flowers dotted over the crown: colour and share, by season. */
  flecks?: Partial<Record<Season, [string, number]>>;
}

type Tree = Conifer | Broadleaf;

const TREES = {
  'norway-spruce': {
    form: 'conifer',
    name: 'Norway spruce',
    base: 0.06,
    radius: 0.17,
    shape: (t) => (1 - t) ** 0.95,
    overlap: 1.9,
    droop: 0.16,
    lobes: 7,
    jitter: 0.08,
    trunk: 0.018,
    flare: 1.8,
    bark: ['#4f3426', '#6e4a35'],
    needles: {
      winter: ['#1f3a33', '#2d5444', '#557a56'],
      spring: ['#24453a', '#33664b', '#9cc35c'],
      summer: ['#22433a', '#33664b', '#6f9c58'],
      autumn: ['#22413a', '#31604a', '#638f52'],
    },
  },
  'silver-fir': {
    form: 'conifer',
    name: 'Silver fir',
    base: 0.1,
    radius: 0.2,
    // Columnar, with the blunt top of an older fir.
    shape: (t) => 1 - t ** 2.2,
    overlap: 1.6,
    droop: 0.06,
    lobes: 8,
    jitter: 0.06,
    trunk: 0.02,
    flare: 1.6,
    bark: ['#5a5852', '#8a877e'],
    needles: {
      winter: ['#203d33', '#2f5644', '#4d7553'],
      spring: ['#264a3d', '#3a6e50', '#8fbf68'],
      summer: ['#244639', '#386a4e', '#76a25e'],
      autumn: ['#23443a', '#36664c', '#6c9658'],
    },
    under: '#86a597',
  },
  'european-larch': {
    form: 'conifer',
    name: 'European larch',
    base: 0.08,
    radius: 0.24,
    shape: (t) => (1 - t) ** 0.85,
    overlap: 0.95,
    droop: 0.35,
    lobes: 5,
    jitter: 0.25,
    trunk: 0.017,
    flare: 1.7,
    bark: ['#5e3a28', '#7a4e36'],
    needles: {
      winter: 'bare',
      spring: ['#5f9447', '#8fc25e', '#c6e58d'],
      summer: ['#4b7a3c', '#6d9e4c', '#a5c96f'],
      autumn: ['#9a6a24', '#c99632', '#ecc458'],
    },
    twigs: ['#5e5048', '#7d6c60', '#9a887a'],
  },
  'douglas-fir': {
    form: 'conifer',
    name: 'Douglas fir',
    base: 0.3,
    radius: 0.18,
    shape: (t) => (1 - t) ** 0.8,
    overlap: 1.7,
    droop: 0.14,
    lobes: 6,
    jitter: 0.3,
    trunk: 0.022,
    flare: 1.6,
    bark: ['#3e271c', '#5a3a2a'],
    needles: {
      winter: ['#1e3a33', '#2c5446', '#4d7458'],
      spring: ['#22443a', '#34654e', '#86b06a'],
      summer: ['#20423a', '#33624d', '#6a9460'],
      autumn: ['#20403a', '#325f4b', '#64905c'],
    },
  },
  'western-hemlock': {
    form: 'conifer',
    name: 'Western hemlock',
    base: 0.08,
    radius: 0.16,
    shape: (t) => 1 - t,
    overlap: 1.9,
    droop: 0.24,
    lobes: 8,
    jitter: 0.1,
    nod: 0.06,
    trunk: 0.017,
    flare: 1.7,
    bark: ['#4a3024', '#6a4a3a'],
    needles: {
      winter: ['#203d30', '#2f5a42', '#5a8256'],
      spring: ['#264a3a', '#3a7050', '#9cc76e'],
      summer: ['#244736', '#386c4c', '#7cab60'],
      autumn: ['#234534', '#36684a', '#74a25c'],
    },
  },
  'western-red-cedar': {
    form: 'conifer',
    name: 'Western red cedar',
    base: 0.1,
    radius: 0.24,
    shape: (t) => (1 - t) ** 1.3,
    overlap: 1.8,
    droop: 0.34,
    lobes: 9,
    jitter: 0.12,
    trunk: 0.03,
    // The buttressed foot of an old cedar.
    flare: 2.6,
    bark: ['#5e2f1e', '#8a4e34'],
    needles: {
      winter: ['#2a4a2e', '#40683c', '#6f9050'],
      spring: ['#2e5432', '#4a7a40', '#a4c462'],
      summer: ['#2d5131', '#47763f', '#8db458'],
      autumn: ['#2c4f30', '#45733e', '#86ad54'],
    },
  },
  'sycamore-maple': {
    form: 'broadleaf',
    name: 'Sycamore maple',
    fork: 0.32,
    crown: [0.6, 0.42, 0.42, 0.38],
    trunk: 0.05,
    flare: 1.5,
    bark: ['#5f564d', '#8a7e70'],
    leaves: {
      winter: 'bare',
      spring: ['#4f7a35', '#78a84a', '#b2d070'],
      summer: ['#365f2c', '#4f7f38', '#7aa651'],
      autumn: ['#7a5a26', '#b08a36', '#dcb752'],
    },
    twigs: ['#6e625a', '#8e8076', '#a89a8e'],
  },
  'european-beech': {
    form: 'broadleaf',
    name: 'European beech',
    fork: 0.4,
    crown: [0.66, 0.34, 0.34, 0.34],
    trunk: 0.04,
    flare: 1.4,
    bark: ['#6f716c', '#a2a29a'],
    leaves: {
      // Beech holds its dead leaves through the winter.
      winter: ['#6a4026', '#8a5a34', '#a8764a'],
      spring: ['#6aa63e', '#94c855', '#c9e98a'],
      summer: ['#335c28', '#4a7c35', '#73a24c'],
      autumn: ['#7e3a18', '#b45e26', '#e08a3c'],
    },
    twigs: ['#5e5a56', '#7c7670', '#97908a'],
  },
  hawthorn: {
    form: 'broadleaf',
    name: 'Hawthorn, wind-bent',
    fork: 0.3,
    crown: [0.55, 0.6, 0.42, 0.24],
    // Leans downwind of the Atlantic westerlies, so it must keep its heading when placed.
    sweep: 0.8,
    trunk: 0.06,
    flare: 1.3,
    bark: ['#4a3d36', '#6a5a50'],
    leaves: {
      winter: 'bare',
      spring: ['#3a5e2c', '#557d3a', '#86a85a'],
      summer: ['#2f5226', '#456e33', '#6c9448'],
      autumn: ['#5a5a2e', '#7a7038', '#9a8a4a'],
    },
    twigs: ['#4c3f3a', '#665650', '#80706a'],
    flecks: { spring: ['#f6f1e6', 0.55], autumn: ['#a3242a', 0.35] },
  },
  gorse: {
    form: 'broadleaf',
    name: 'Gorse',
    fork: 0,
    crown: [0.5, 0.85, 0.75, 0.5],
    trunk: 0,
    flare: 1,
    bark: ['#4a3d36', '#6a5a50'],
    leaves: {
      winter: ['#28391f', '#3b5229', '#5c7239'],
      spring: ['#2e4424', '#435e2e', '#6a8042'],
      summer: ['#2e4424', '#435e2e', '#6a8042'],
      autumn: ['#2c4022', '#40592c', '#647a40'],
    },
    twigs: ['#4c3f3a', '#665650', '#80706a'],
    // Gorse flowers a little in every month and floods yellow in spring.
    flecks: { winter: ['#f2c230', 0.25], spring: ['#f6c62c', 0.6], summer: ['#f2c230', 0.2], autumn: ['#f2c230', 0.12] },
  },
} satisfies Record<string, Tree>;

export type TreeName = keyof typeof TREES;
export const TREE_NAMES = Object.keys(TREES) as TreeName[];

export const treeLabel = (name: TreeName) => TREES[name].name;

export function tree(name: TreeName, season: Season, lod: Lod, seed = 1): THREE.BufferGeometry {
  const spec: Tree = TREES[name];
  return unit(spec.form === 'conifer' ? conifer(spec, season, lod, seed) : broadleaf(spec, season, lod, seed));
}

const ramp = (r: Ramp) => r.map(hex) as [THREE.Color, THREE.Color, THREE.Color];

/** A trunk from the ground to `top`, darker at its foot. */
function trunk({ trunk: r, flare, bark }: Bark, top: number, lod: Lod, seed: number): THREE.BufferGeometry {
  const [sides, rows] = DETAIL[lod].trunk;
  const [foot, bole] = bark.map(hex);
  const profile: [number, number][] = [
    [0, 0],
    [r * flare, 0],
    [(r * (1 + flare)) / 2, 2 * r],
    [r, Math.min(6 * r, 0.5 * top)],
    [r * 0.6, top],
    [0, top + r],
  ];
  const geo = soft(lathe(profile, sides, rows), 0.075 * r, 1.4 / r, seed);
  return paint(geo, (p) => foot.clone().lerp(bole, smooth(0, 0.5 * top, p.z)));
}

// One tier of branches, radius 1 and height 1: out along the underside to the rim, then up to the tip.
const TIER: [number, number][] = [[0, -0.04], [0.45, -0.08], [0.86, -0.12], [1, -0.04], [0.97, 0.05], [0.78, 0.2], [0.42, 0.52], [0.12, 0.87], [0, 1]];

/** Droops a tier's rim into branch tips. Whole lobes keep the lathe seam closed. */
function scallop(geo: THREE.BufferGeometry, lobes: number, droop: number, seed: number): THREE.BufferGeometry {
  const pos = geo.getAttribute('position');
  for (let i = 0; i < pos.count; i++) {
    const [x, y, z] = [pos.getX(i), pos.getY(i), pos.getZ(i)];
    const rim = smooth(0.55, 0.95, Math.hypot(x, y));
    const tip = 0.5 + 0.5 * Math.cos(Math.atan2(y, x) * lobes + seed);
    const grow = 1 + 0.08 * rim * (tip - 0.4);
    pos.setXYZ(i, x * grow, y * grow, z - droop * rim * tip);
  }
  return geo;
}

/** Stacked tiers along the crown; fewer, taller tiers keep the same outline at a lower detail. */
function conifer(spec: Conifer, season: Season, lod: Lod, seed: number): THREE.BufferGeometry {
  const { sides, rows, tiers } = DETAIL[lod];
  const rand = random(seed);
  const own = spec.needles[season];
  const bare = own === 'bare';
  const [deep, mid, tip] = ramp(bare ? spec.twigs! : own);
  const under = spec.under ? hex(spec.under) : deep;
  const needles = (p: THREE.Vector3, n: THREE.Vector3) =>
    deep
      .clone()
      .lerp(mid, smooth(-0.2, 0.6, n.z))
      .lerp(under, 0.5 * smooth(-0.4, -0.9, n.z))
      .lerp(tip, 0.6 * smooth(0.35, 1, p.z) * smooth(0, 0.5, n.z));
  // A tier with more tips than corners would droop on one side only.
  const lobes = Math.min(spec.lobes, Math.floor(sides / 2));
  const step = (1 - spec.base) / (tiers - 1 + spec.overlap);
  const height = spec.overlap * step;
  const branches = bare && lod === 0;
  const parts = [trunk(spec, branches ? 0.97 : 1 - height, lod, seed)];
  for (let i = 0; i < tiers; i++) {
    const z = spec.base + i * step;
    const r = spec.radius * spec.shape((z - spec.base) / (1 - spec.base)) * (1 + spec.jitter * (2 * rand() - 1));
    const shift = spec.jitter * r * (2 * rand() - 1);
    const spin = rand() * Math.PI * 2;
    if (branches) {
      // A whorl of bare branches in the place of the tier, drooping as far.
      for (let k = 0; k < 5; k++) {
        const a = spin + (k / 5) * 2 * Math.PI;
        const end: Vec = [shift + r * Math.cos(a), r * Math.sin(a), z - spec.droop * height];
        parts.push(limb([0, 0, z + 0.3 * height], end, spec.trunk * 0.3, mid, 0, 4));
      }
      continue;
    }
    const geo = soft(scallop(lathe(TIER, sides, rows), lobes, spec.droop, seed + i), 0.02, 4, seed + i);
    parts.push(place(paint(geo, needles), { at: [shift, shift * 0.5, z], rot: [0, 0, spin], scale: [r, r, height] }));
  }
  const geo = merge(parts);
  if (spec.nod) nod(geo, spec.nod);
  return geo;
}

/** Bends the top of the leader over towards +x. */
function nod(geo: THREE.BufferGeometry, amount: number): void {
  const pos = geo.getAttribute('position');
  const from = 1 - 3 * amount;
  for (let i = 0; i < pos.count; i++) {
    const k = Math.max(0, pos.getZ(i) - from) / (1 - from);
    pos.setXYZ(i, pos.getX(i) + amount * k * k, pos.getY(i), pos.getZ(i) - 0.6 * amount * k * k * k);
  }
  geo.computeVertexNormals();
}

/** Crown lumps in the crown's own frame, radius 1: a core, an optional top and a ring around. */
function lumps(count: number, rand: () => number): { at: THREE.Vector3; r: number }[] {
  const out = [{ at: v3(), r: 0.72 }];
  if (count >= 6) out.push({ at: v3(0, 0, 0.5), r: 0.5 });
  const ring = count - out.length;
  for (let k = 0; k < ring; k++) {
    const a = (k / ring) * 2 * Math.PI + rand() * 0.5;
    const z = (k % 2 ? 0.2 : -0.15) + (rand() - 0.5) * 0.15;
    out.push({ at: v3(0.55 * Math.cos(a), 0.55 * Math.sin(a), z), r: 0.48 + rand() * 0.08 });
  }
  return out;
}

/** Dots on `share` of the vertices, fewer underneath. Far away an even tint, where dots would flicker. */
function fleck(p: THREE.Vector3, n: THREE.Vector3, share: number, lod: Lod, seed: number): number {
  const dot = hash(p, seed) < share ? 0.6 + 0.4 * smooth(-0.5, 0.5, n.z) : 0;
  return THREE.MathUtils.lerp(dot, 0.6 * share, lod / 2);
}

/**
 * A crown of soft lumps on a trunk. A near bare tree shows its limbs and twigs; a far one keeps
 * its crown in the colour of the twigs.
 */
function broadleaf(spec: Broadleaf, season: Season, lod: Lod, seed: number): THREE.BufferGeometry {
  const { lumps: count, ico } = DETAIL[lod];
  const rand = random(seed);
  const [cz, rx, ry, rz] = spec.crown;
  const toTree = (p: THREE.Vector3) => v3(p.x * rx, p.y * ry, cz + p.z * rz);
  const leaves = spec.leaves[season];
  const bare = leaves === 'bare';
  const [deep, mid, tip] = ramp(bare ? spec.twigs : leaves);
  const flecks = spec.flecks?.[season];
  const foliage = (p: THREE.Vector3, n: THREE.Vector3) => {
    const c = deep
      .clone()
      .lerp(mid, 0.7 * smooth(-0.6, 0.3, n.z) + 0.3 * smooth(-0.8, 0.6, p.z))
      .lerp(tip, 0.7 * smooth(0.2, 0.9, n.z) * smooth(-0.2, 0.8, p.z));
    return flecks ? c.lerp(hex(flecks[0]), fleck(p, n, flecks[1], lod, seed)) : c;
  };
  const crown = lumps(count, rand);
  const parts: THREE.BufferGeometry[] = [];
  if (spec.trunk) parts.push(trunk(spec, spec.fork + 0.3 * rz, lod, seed));
  if (!(bare && lod === 0)) {
    crown.forEach(({ at, r }, k) => {
      // Each lump a little lighter or darker, so the crown does not read as one blob.
      const tone = 0.9 + 0.2 * rand();
      const geo = paint(place(soft(lump(r, ico), 0.04, 4, seed + k), { at }), (p, n) => foliage(p, n).multiplyScalar(tone));
      parts.push(place(geo, { at: [0, 0, cz], scale: [rx, ry, rz] }));
    });
  }
  if (spec.trunk && lod === 0) {
    // Limbs run from the fork into the outer lumps; bare ones end in twigs.
    const [foot, bole] = spec.bark.map(hex);
    const [, twig, twigTip] = ramp(spec.twigs);
    const fork: Vec = [0, 0, spec.fork];
    for (const { at, r } of crown.slice(1)) {
      const end = toTree(at.clone().multiplyScalar(bare ? 1 : 0.8));
      parts.push(limb(fork, end.toArray(), spec.trunk * 0.45, (p) => foot.clone().lerp(bole, smooth(0, spec.fork, p.z)), 0, 6));
      if (!bare) continue;
      for (let k = 0; k < 3; k++) {
        const a = rand() * Math.PI * 2;
        const out = at.clone().normalize().add(v3(0.6 * Math.cos(a), 0.6 * Math.sin(a), 0.5)).setLength(r * 0.9);
        const tipAt = toTree(at.clone().add(out));
        parts.push(limb(end.toArray(), tipAt.toArray(), spec.trunk * 0.18, (p) => twig.clone().lerp(twigTip, smooth(cz, cz + rz, p.z)), 0, 4));
      }
    }
  }
  const geo = merge(parts);
  if (spec.sweep) sweep(geo, spec.sweep);
  return geo;
}

/** Shears a tree downwind, towards +x, more the higher it reaches. */
function sweep(geo: THREE.BufferGeometry, amount: number): void {
  const pos = geo.getAttribute('position');
  for (let i = 0; i < pos.count; i++) pos.setX(i, pos.getX(i) + amount * pos.getZ(i) ** 2);
  geo.computeVertexNormals();
}
