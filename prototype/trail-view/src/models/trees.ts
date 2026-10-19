/**
 * The tree builder: a species file in trees/ says what a tree is, and this draws it at three levels
 * of detail. A tree is 1 tall with its trunk base at the origin and z up; the instanced shader
 * scales it by the tree's height. The season changes vertex colours, and a bare tree's crown.
 */

import * as THREE from 'three';
import { domed, GOLDEN, hash, hex, lathe, limb, lump, merge, paint, place, ramp, random, sheet, smooth, soft, unit, v3, type Ramp, type Sheet, type Vec } from './shape';

/** The order of SEASONS in light.ts. */
export const SEASONS = ['winter', 'spring', 'summer', 'autumn'] as const;
export type Season = (typeof SEASONS)[number];

/** 0 within 40 m, 1 within 200 m, 2 out to 1 km. */
export type Lod = 0 | 1 | 2;
export const LOD_RANGE_M = [40, 200, 1000] as const;

/**
 * Segments around a part, spline rows per tier, conifer tiers and broadleaf crown lumps. A trunk
 * has its own sides and rows per profile span. A bare crown has `twigs` twigs, each `width` wide
 * at its root in tree heights: fewer and wider at a level whose trees are further off, so the haze
 * keeps its density from level to level. Its limbs have `limbs` sides, or are flat at 0.
 */
const DETAIL = [
  { sides: 22, rows: 10, tiers: 7, trunk: [10, 2], lumps: 9, ico: 3, twigs: 400, width: 0.004, limbs: 6 },
  { sides: 10, rows: 6, tiers: 5, trunk: [6, 1], lumps: 6, ico: 1, twigs: 380, width: 0.006, limbs: 4 },
  { sides: 6, rows: 3, tiers: 3, trunk: [4, 1], lumps: 5, ico: 0, twigs: 95, width: 0.025, limbs: 0 },
] as const;

interface Bark {
  /** Trunk radius at the ground, and the root flare in trunk radii. */
  trunk: number;
  flare: number;
  /** The trunk's radius at its top, in ground radii; 0.6 by default. */
  neck?: number;
  /** Colour at the ground and higher up. */
  bark: [string, string];
  /** Where the ground colour turns into the colour higher up, in shares of the trunk; [0, 0.5] by default. */
  fade?: [number, number];
  /** Dark marks on pale bark, as on birch: their colour and share of the bark. */
  marks?: [string, number];
  /** Diamond leaf scars, as on a palm: how many around the trunk. */
  scars?: number;
}

interface Base extends Bark {
  name: string;
  latin: string;
  /** Placed at the heading it is built in, as a tree the wind bends must be; others turn at random. */
  keepsHeading?: true;
}

interface Conifer extends Base {
  form: 'conifer';
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
  /** The colour of a bare conifer's branches and twigs. */
  twigs?: Ramp;
  /** Pale needle undersides, as on silver fir. */
  under?: string;
  /** How far the leader nods over towards +x, in tree heights. */
  nod?: number;
  /** Each tier a whorl of branches ending in drooping tufts, with sky between, as on a Canary pine. */
  tufts?: true;
  /** Needle tufts sprouting from the bare trunk: how many a near tree has. */
  sprouts?: number;
}

interface Broadleaf extends Base {
  form: 'broadleaf';
  /** Where the trunk divides. A bush has no trunk. */
  fork: number;
  /** Crown centre height and half-sizes along x, y and z. */
  crown: [number, number, number, number];
  /** How far the top is blown over towards +x, in tree heights. */
  sweep?: number;
  /** An open crown of separate flat puffs on lifted branches, as on a pine, instead of a dome. */
  open?: true;
  /** Limbs leave the trunk at staggered heights below their lumps, as on an old pine, not from one fork. */
  stagger?: true;
  /** Puffs are flat-topped pads, as on a Scots pine; a near tree has three more. */
  pads?: true;
  /** Lumps hang into points this far, in lump radii, as the twigs of a weeping birch. */
  weep?: number;
  /** Trunks leaning out from one foot. */
  stems?: number;
  /** A palm: this many fronds near the eye arch out of the trunk's top, as long as the crown is wide. */
  fronds?: number;
  /** A dragon tree: limbs fork this many ways at each level and end in rosettes of sword leaves. */
  branching?: number[];
  leaves: Record<Season, Ramp | 'bare'>;
  /** The colour of a bare crown's twigs. */
  twigs: Ramp;
  /**
   * Blossom, berries or flowers dotted over the crown, or on a bare crown whole twigs, as catkins or
   * dead leaves that hang on: colour and share, by season.
   */
  flecks?: Partial<Record<Season, [string, number]>>;
}

/** A species file in trees/. */
export type Tree = Conifer | Broadleaf;

export function grow(spec: Tree, season: Season, lod: Lod, seed: number): THREE.BufferGeometry {
  return unit(spec.form === 'conifer' ? conifer(spec, season, lod, seed) : broadleaf(spec, season, lod, seed));
}

/** Bands of marks up a near marked trunk; each holds up to two dashes. */
const BANDS = 18;

/** A trunk from the ground to `top`, darker at its foot. */
function trunk({ trunk: r, flare, neck = 0.6, bark, fade = [0, 0.5], marks, scars }: Bark, top: number, lod: Lod, seed: number): THREE.BufferGeometry {
  if (scars && lod === 0) return scarred(r, flare, bark, scars, top);
  const [sides, perSpan] = DETAIL[lod].trunk;
  const [foot, bole] = bark.map(hex);
  const low = Math.min(6 * r, 0.5 * top);
  const at = (z: number): [number, number] => [r * THREE.MathUtils.lerp(1, neck, (z - low) / (top - low)), z];
  // A near marked trunk has a row at each edge of each band of marks, which are rows enough.
  const bands = marks && lod === 0 ? BANDS : 0;
  const centre = (b: number) => low + ((b + 0.5) / bands) * (top - low);
  const half = 0.3 * r;
  const profile: [number, number][] = [
    [0, 0],
    [r * flare, 0],
    // A far trunk's flare is one straight span.
    ...(lod < 2 ? [[(r * (1 + flare)) / 2, 2 * r] as [number, number]] : []),
    ...(bands ? Array.from({ length: bands }, (_, b) => [at(centre(b) - half), at(centre(b) + half)]).flat() : [at(low)]),
    at(top),
    [0, top + r],
  ];
  // The lathe samples its spline evenly per span, so whole rows per span keep every profile point.
  const geo = soft(lathe(profile, sides, (bands ? 1 : perSpan) * (profile.length - 1)), 0.075 * r, 1.4 / r, seed);
  // A far scarred trunk takes the mean shade of its plates and grooves.
  const colour = (p: THREE.Vector3) => foot.clone().lerp(bole, smooth(fade[0] * top, fade[1] * top, p.z)).multiplyScalar(scars ? 0.8 : 1);
  if (!marks) return paint(geo, colour);
  const mark = hex(marks[0]);
  if (!bands) return paint(geo, (p) => colour(p).lerp(mark, 0.3 * marks[1]));
  // Dashes an eighth to a quarter of the way round.
  const rand = random(seed);
  const dashes = Array.from({ length: bands }, () => [0, 1].flatMap(() => (rand() < marks[1] ? [[2 * Math.PI * rand(), (Math.PI / 8) * (1 + rand())]] : [])));
  return stain(paint(geo, colour), mark, (c) => {
    const b = Math.round(((c.z - low) / (top - low)) * bands - 0.5);
    const a = Math.atan2(c.y, c.x);
    return Math.abs(c.z - centre(b)) < half && (dashes[b] ?? []).some(([to, width]) => Math.abs(((a - to + 3 * Math.PI) % (2 * Math.PI)) - Math.PI) < width);
  });
}

/** Paints whole faces whose centre `dark` picks, so marks keep the sharp edges vertex colours would blur. */
function stain(geo: THREE.BufferGeometry, mark: THREE.Color, dark: (centre: THREE.Vector3) => boolean): THREE.BufferGeometry {
  const flat = geo.toNonIndexed();
  const pos = flat.getAttribute('position');
  const col = flat.getAttribute('color');
  const c = v3();
  for (let i = 0; i < pos.count; i += 3) {
    c.set(pos.getX(i) + pos.getX(i + 1) + pos.getX(i + 2), pos.getY(i) + pos.getY(i + 1) + pos.getY(i + 2), pos.getZ(i) + pos.getZ(i + 1) + pos.getZ(i + 2)).divideScalar(3);
    if (dark(c)) for (let j = i; j < i + 3; j++) col.setXYZ(j, mark.r, mark.g, mark.b);
  }
  return flat.setIndex(Array.from({ length: pos.count }, (_, i) => i));
}

/**
 * A trunk of diamond leaf scars in rings, each ring turned half a diamond. A diamond is four flat
 * faces around a raised centre, so the sun lights its upper half and the grooves stay dark:
 * 4 × n triangles a ring.
 */
function scarred(r: number, flare: number, bark: [string, string], n: number, top: number): THREE.BufferGeometry {
  const [foot, bole] = bark.map(hex);
  // Rings half a diamond apart; a diamond is half as tall as it is wide.
  const h = (0.25 * 2 * Math.PI * r) / n;
  const corner = (i: number, k: number, raised = false) => {
    const z = i * h;
    // Scars run in a slow spiral, as the leaves grew.
    const a = (k / 2 + 0.03 * i) * ((2 * Math.PI) / n);
    const radius = r * (1 + (flare - 1) * (1 - smooth(0, 4 * r, z))) * (raised ? 1.15 : 1);
    return v3(radius * Math.cos(a), radius * Math.sin(a), z);
  };
  const cards = sheet();
  for (let i = 0; i * h < top; i++) {
    for (let k = i % 2; k < 2 * n; k += 2) {
      const c = corner(i, k, true);
      const ring = [corner(i, k + 1), corner(i + 1, k), corner(i, k - 1), corner(i - 1, k)];
      const plate = foot.clone().lerp(bole, smooth(0, 0.5 * top, c.z));
      const groove = plate.clone().multiplyScalar(0.55);
      for (let e = 0; e < 4; e++) cards.add([c, ring[e], ring[(e + 1) % 4]], [plate, groove, groove]);
    }
  }
  return cards.build();
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
  const { sides, rows } = DETAIL[lod];
  // A far tufted pine keeps its whorls, as its tiers are layers apart, not one cone.
  const { tiers } = DETAIL[spec.tufts ? Math.min(lod, 1) : lod];
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
  const parts = [trunk(spec, bare ? 0.97 : 1 - height, lod, seed)];
  const twigs = sheet();
  const bole = hex(spec.bark[1]);
  // A flat ball of needles drooping from a branch tip: 80 triangles near, 20, then a 12-triangle
  // double cone. A far tuft has too few corners to droop without turning into a point.
  const tuft = (at: Vec, size: number, k: number) => {
    const geo = lod === 2 ? soft(lathe([[0, -1], [1, 0], [0, 1]], 6, 2)) : soft(lump(1, lod ? 0 : 1), 0.08, 3, seed + k);
    if (lod < 2) hang(geo, 1, spec.droop);
    return place(paint(geo, needles), { at, scale: [size, size, 0.5 * size] });
  };
  for (let i = 0; i < tiers; i++) {
    const z = spec.base + i * step;
    const r = spec.radius * spec.shape((z - spec.base) / (1 - spec.base)) * (1 + spec.jitter * (2 * rand() - 1));
    const shift = spec.jitter * r * (2 * rand() - 1);
    const spin = rand() * Math.PI * 2;
    if (bare) {
      // A whorl of bare branches in the place of the tier, drooping as far, twigs hanging from them.
      for (let k = 0; k < 5; k++) {
        const a = spin + (k / 5) * 2 * Math.PI;
        const from = v3(0, 0, z + 0.3 * height);
        const end = v3(shift + r * Math.cos(a), r * Math.sin(a), z - spec.droop * height);
        // Branches thinner than a broadleaf's limbs go flat a level sooner.
        bough(parts, twigs, from, end, spec.trunk * 0.3, () => mid, DETAIL[lod + 1]?.limbs ?? 0, rand);
        // A conifer's crown is narrow, so each branch has a thirtieth of a broadleaf crown's twigs.
        for (let j = 0; j < DETAIL[lod].twigs / 30; j++) {
          const root = from.clone().lerp(end, 0.3 + 0.7 * rand());
          const out = v3(rand() - 0.5, rand() - 0.5, rand() - 0.5).normalize().add(v3(Math.cos(a), Math.sin(a), -0.6)).normalize();
          twig(twigs, root, root.clone().addScaledVector(out, 0.4 * r), DETAIL[lod].width, [mid, tip], rand);
        }
      }
      continue;
    }
    if (spec.tufts && lod === 2) {
      // Far off a whorl's tufts merge into one flat layer.
      parts.push(place(tuft([0, 0, 0], 1, i), { at: [shift, 0, z + 0.2 * height], rot: [0, 0, spin], scale: [1.25 * r, 1.25 * r, 0.85 * r] }));
      continue;
    }
    if (spec.tufts) {
      const n = spec.lobes;
      for (let k = 0; k < n; k++) {
        const a = spin + ((k + 0.5 * rand()) / n) * 2 * Math.PI;
        const end: Vec = [shift + 0.8 * r * Math.cos(a), 0.8 * r * Math.sin(a), z + 0.2 * height];
        parts.push(limb([0, 0, z], end, spec.trunk * 0.3, bole, 0, 4));
        parts.push(tuft(end, 0.45 * r * (1 + 0.2 * lod), i * n + k));
      }
      continue;
    }
    const geo = soft(scallop(lathe(TIER, sides, rows), lobes, spec.droop, seed + i), 0.02, 4, seed + i);
    parts.push(place(paint(geo, needles), { at: [shift, shift * 0.5, z], rot: [0, 0, spin], scale: [r, r, height] }));
  }
  if (spec.tufts && !bare) parts.push(tuft([0, 0, 1 - 0.5 * height], 0.35 * spec.radius, -1));
  // Sprouts are too small to see beyond the near trees.
  for (let k = 0; !bare && k < (spec.sprouts ?? 0) >> (2 * lod); k++) {
    const a = rand() * 2 * Math.PI;
    const size = spec.trunk * (0.8 + 0.6 * rand());
    const at: Vec = [1.2 * spec.trunk * Math.cos(a), 1.2 * spec.trunk * Math.sin(a), spec.base * (0.25 + 0.7 * rand())];
    parts.push(tuft(at, size, 100 + k));
  }
  if (bare) parts.push(twigs.build());
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

/**
 * An open crown in its own frame, radius 1: a puff on top and smaller ones around it, with sky
 * between. More puffs are smaller; fewer huddle in a little, as a far tree has no limbs to hold them.
 */
function puffs(count: number, rand: () => number): { at: THREE.Vector3; r: number }[] {
  const out = [{ at: v3(0.1, 0, 0.5), r: 0.5 }];
  const ring = count - 1;
  for (let k = 0; k < ring; k++) {
    const a = (k / ring) * 2 * Math.PI + rand() * 0.6;
    const d = THREE.MathUtils.clamp(ring / 5, 0.85, 1) * (0.62 + 0.25 * rand());
    out.push({ at: v3(d * Math.cos(a), d * Math.sin(a), -0.5 + 0.9 * rand()), r: Math.min(1, Math.sqrt(5 / ring)) * (0.32 + 0.1 * rand()) });
  }
  return out;
}

/** Dots on `share` of the vertices, fewer underneath. Far away an even tint, where dots would flicker. */
function fleck(p: THREE.Vector3, n: THREE.Vector3, share: number, lod: Lod, seed: number): number {
  const dot = hash(p, seed) < share ? 0.6 + 0.4 * smooth(-0.5, 0.5, n.z) : 0;
  return THREE.MathUtils.lerp(dot, 0.6 * share, lod / 2);
}

/**
 * A crown of soft lumps on a trunk. A bare crown is a limb into each lump and twigs from along it
 * out to the lump's far side, with sky between them, so its outline stays at every distance.
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
  // A palm or a dragon tree grows its crown from the trunk's top.
  if (spec.fronds || spec.branching) {
    const crown = spec.fronds ? palm(spec, lod, [deep, mid, tip], rand) : dragon(spec, lod, [deep, mid, tip], rand);
    return merge([trunk(spec, spec.fork, lod, seed), ...crown]);
  }
  const crown = spec.open ? puffs(count + (spec.pads && !lod ? 3 : 0), rand) : lumps(count, rand);
  const parts: THREE.BufferGeometry[] = [];
  // An open crown's trunk runs up into its top puff, which no limb reaches.
  const top = spec.open ? cz + 0.3 * rz : spec.fork + 0.3 * rz;
  // A far tree's stems hide under its crown, so it has one trunk as stout as all of them.
  const stems = lod < 2 ? (spec.stems ?? 1) : 1;
  const stout = { ...spec, trunk: spec.trunk * Math.sqrt((spec.stems ?? 1) / stems) };
  for (let s = 0; spec.trunk && s < stems; s++) {
    const a = (s / stems) * 2 * Math.PI;
    // Stems lean apart from one stool, their feet a little apart.
    const out = stems > 1 ? v3(Math.cos(a), Math.sin(a), 0) : v3();
    const lean = out.clone().multiplyScalar(0.3).setZ(1).normalize();
    parts.push(place(trunk(stout, top, lod, seed + s), { at: out.multiplyScalar(1.2 * spec.trunk), quat: new THREE.Quaternion().setFromUnitVectors(v3(0, 0, 1), lean) }));
  }
  const [foot, bole] = spec.bark.map(hex);
  const bark = (p: THREE.Vector3) => foot.clone().lerp(bole, smooth(0, spec.fork, p.z));
  if (bare) {
    const twigs = sheet();
    const [dead, share] = flecks ? [hex(flecks[0]), flecks[1]] : [mid, 0];
    const weep = spec.weep ?? 0;
    for (const { at, r } of crown) {
      const out = at.clone().normalize();
      const end = toTree(at.clone().addScaledVector(out, 0.5 * r));
      // No limb leaves the trunk above its top, so the top lump of an open crown grows from it.
      const from = v3(0, 0, spec.stagger ? Math.min(top, Math.max(spec.fork, end.z - 0.6 * Math.hypot(end.x, end.y))) : spec.fork);
      bough(parts, twigs, from, end, spec.trunk * 0.45, bark, DETAIL[lod].limbs, rand);
      for (let k = 0; k < DETAIL[lod].twigs / crown.length; k++) {
        const root = from.clone().lerp(end, 0.5 + 0.5 * rand());
        // Twigs reach out to the lump's far side; a weeping tree's droop to its underside and hang below.
        const reach = toTree(at.clone().addScaledVector(v3(rand() - 0.5, rand() - 0.5, rand() - 0.5 - weep).normalize().add(out).normalize(), r));
        reach.z -= weep * rand() * r * rz;
        twig(twigs, root, reach, DETAIL[lod].width, rand() < share ? [dead, dead] : [mid, tip], rand);
      }
    }
    parts.push(twigs.build());
  } else {
    crown.forEach(({ at, r }, k) => {
      // Each lump a little lighter or darker, so the crown does not read as one blob.
      const tone = 0.9 + 0.2 * rand();
      // A flat pad needs fewer corners than a round puff.
      const puff = soft(lump(r, spec.pads ? Math.min(ico, 2) : ico), 0.04, 4, seed + k);
      if (spec.weep) hang(puff, r, spec.weep);
      if (spec.pads) pad(puff, r);
      const geo = paint(place(puff, { at }), (p, n) => foliage(p, n).multiplyScalar(tone));
      parts.push(place(geo, { at: [0, 0, cz], scale: [rx, ry, rz] }));
    });
    // Limbs run from the fork into the outer lumps. An open crown keeps them a level further out,
    // so its puffs never float.
    if (spec.trunk && (lod === 0 || (spec.open && lod === 1))) {
      const fork: Vec = [0, 0, spec.fork];
      for (const { at } of crown.slice(1)) {
        const end = toTree(at.clone().multiplyScalar(0.8));
        const from: Vec = spec.stagger ? [0, 0, Math.max(spec.fork, end.z - 0.6 * Math.hypot(end.x, end.y))] : fork;
        parts.push(limb(from, end.toArray(), spec.trunk * 0.45, bark, 0, lod ? 4 : 6));
      }
    }
  }
  const geo = merge(parts);
  if (spec.sweep) sweep(geo, spec.sweep);
  return geo;
}

/** A bare limb: round with `sides` sides, or at 0 sides one flat triangle among the twigs. */
function bough(parts: THREE.BufferGeometry[], twigs: Sheet, from: THREE.Vector3, to: THREE.Vector3, radius: number, bark: (p: THREE.Vector3) => THREE.Color, sides: number, rand: () => number): void {
  if (sides) parts.push(limb(from.toArray(), to.toArray(), radius, bark, 0, sides));
  else twig(twigs, from, to, 2 * radius, [bark(from), bark(to)], rand);
}

/**
 * A twig, or a flat limb: one upright triangle from `width` across at its root to a point at its
 * tip. Upright, it shows from the side and its normal stays level; it faces either way, so half the
 * twigs catch the sun wherever they are in the crown.
 */
function twig(twigs: Sheet, root: THREE.Vector3, tip: THREE.Vector3, width: number, [base, end]: THREE.Color[], rand: () => number): void {
  const along = v3().subVectors(tip, root);
  const across = v3(0, 0, 1).addScaledVector(along, -along.z / along.lengthSq());
  if (across.lengthSq() < 1e-6) across.set(1, 0, 0);
  across.setLength((rand() < 0.5 ? 0.5 : -0.5) * width);
  twigs.add([root.clone().sub(across), root.clone().add(across), tip], [base, base, end]);
}

/**
 * Fronds arch out of a knob of leaf bases, the young ones upright and the old ones hanging. Near
 * the eye each leaflet is one card, so a frond reads as a comb: 48 fronds of 11 pairs. Further off
 * a frond is one strip over the comb's outline, which keeps the crown's mass.
 */
function palm(spec: Broadleaf, lod: Lod, [deep, mid, tip]: THREE.Color[], rand: () => number): THREE.BufferGeometry[] {
  const [cz, length] = spec.crown;
  const count = [spec.fronds!, 36, 16][lod];
  const segments = [11, 4, 2][lod];
  const base = v3(0, 0, spec.fork);
  const light = domed(v3(0, 0, cz), v3(1, 1, 1), 0.3);
  const cards = sheet();
  for (let f = 0; f < count; f++) {
    const age = (f + 0.5) / count;
    const rise = THREE.MathUtils.lerp(1.3, -0.6, age) + 0.15 * (rand() - 0.5);
    const a = f * GOLDEN;
    const out = v3(Math.cos(a), Math.sin(a), 0);
    const side = v3(-Math.sin(a), Math.cos(a), 0);
    const reach = length * (0.85 + 0.25 * rand());
    // Even the upright fronds arch over, so the crown reads as a fountain, not a spray of spikes.
    const sag = 0.25 + 0.25 * Math.cos(rise);
    const along = (s: number) =>
      base.clone().addScaledVector(out, Math.cos(rise) * s * reach).setZ(spec.fork + (Math.sin(rise) * s - sag * s * s) * reach);
    const width = (s: number) => 0.22 * reach * Math.sin(Math.PI * Math.min(1, s + 0.12)) ** 0.7;
    // Old fronds yellow towards the tips.
    const lit = tip.clone().lerp(hex('#c8b860'), 0.6 * smooth(0.6, 1, age));
    const root = (s: number) => deep.clone().lerp(mid, s);
    const rim = (s: number) => mid.clone().lerp(lit, smooth(0.2, 0.9, s));
    for (let j = 0; j < segments; j++) {
      const s = 0.12 + (0.86 * j) / segments;
      // Leaflets leave gaps between them; strip segments meet.
      const t = s + (lod ? 0.86 : 0.8) / segments;
      const [p, q] = [along(s), along(t)];
      const forward = q.clone().sub(p).normalize();
      // Where a leaflet from c ends: out, forward and up.
      const leaflet = (c: THREE.Vector3, s: number, sign: number) =>
        c.clone().addScaledVector(side, sign * width(s)).addScaledVector(forward, 0.4 * width(s)).add(v3(0, 0, 0.3 * width(s)));
      // Darker than the rim, as a comb's roots and the shade between its leaflets are.
      const rib = (s: number) => root(s).lerp(rim(s), 0.25);
      for (const sign of [-1, 1]) {
        const ep = leaflet(p, s, sign);
        if (lod === 0) cards.add([p, q, ep], [root(s), root(s), rim(s)], light);
        else {
          cards.add([p, q, ep], [rib(s), rib(t), rim(s)], light);
          // The tip narrows to a point.
          if (j < segments - 1) cards.add([q, leaflet(q, t, sign), ep], [rib(t), rim(t), rim(s)], light);
        }
      }
    }
  }
  // Leaf bases far off are as green as the fronds around them, lest they read as a nut.
  const knob = paint(place(soft(lump((lod ? 1.3 : 1.7) * spec.trunk, lod ? 0 : 1), 0.1 * spec.trunk, 30), { at: base }), hex('#6a5a3c').lerp(deep, lod ? 0.6 : 0));
  return [cards.build(), knob];
}

/**
 * Limbs fork `branching` ways at each level, rising and spreading so the rosettes of sword
 * leaves at their ends make an umbrella. A tree within 200 m keeps two levels; a far one keeps
 * the first, each limb ending in a ragged bowl where its rosettes would be.
 */
function dragon(spec: Broadleaf, lod: Lod, [deep, mid, tip]: THREE.Color[], rand: () => number): THREE.BufferGeometry[] {
  const [cz, rx, , rz] = spec.crown;
  const levels = spec.branching!.slice(0, [3, 2, 1][lod]);
  const [foot, bole] = spec.bark.map(hex);
  const bark = (p: THREE.Vector3) => foot.clone().lerp(bole, smooth(0, spec.fork, p.z));
  const size = 0.2;
  const blades = lod ? 20 : 32;
  const fork = v3(0, 0, spec.fork);
  // A collar where the limbs leave the trunk, so the trunk swells into them.
  const parts = lod < 2 ? [paint(place(soft(lump(1.1 * spec.trunk, lod ? 0 : 1), 0.08 * spec.trunk, 30), { at: fork }), bark)] : [];
  const cards = sheet();
  const grow = (from: THREE.Vector3, a0: number, a1: number, level: number, r: number) => {
    const m = levels[level];
    for (let c = 0; c < m; c++) {
      const a = a0 + ((c + 0.5 + 0.4 * (rand() - 0.5)) / m) * (a1 - a0);
      // A far tree's one level reaches as far as a near tree's first.
      const f = (level + 1) / Math.max(2, levels.length);
      const d = rx * f ** 0.8 * (0.75 + 0.35 * rand());
      const z = spec.fork + (cz + rz * Math.sqrt(Math.max(0, 1 - (d / rx) ** 2)) - size - spec.fork) * f ** 0.45;
      const to = v3(d * Math.cos(a), d * Math.sin(a), z);
      parts.push(limb(from.toArray(), to.toArray(), r, bark, 0, [6, 4, 3][lod]));
      if (level + 1 < levels.length) grow(to, a0 + (c * (a1 - a0)) / m, a0 + ((c + 1) * (a1 - a0)) / m, level + 1, 0.75 * r);
      else if (lod === 2) bowl(cards, to, a, 0.7 * rx, size, [deep, mid, tip]);
      else {
        rosette(cards, to, size, blades, [deep, mid, tip], rand);
        // A heart fills the rosette between its blades.
        parts.push(paint(place(soft(lump(0.38 * size, 0)), { at: to.clone().setZ(to.z + 0.15 * size) }), deep.clone().lerp(mid, 0.5)));
      }
    }
  };
  grow(fork, 0, 2 * Math.PI, 0, 0.8 * spec.trunk);
  return [...parts, cards.build()];
}

/**
 * A far group of rosettes: a bowl of 8 triangles from a limb's end, out towards `heading`, its
 * rim ragged so its top edge reads as blades.
 */
function bowl(cards: Sheet, at: THREE.Vector3, heading: number, radius: number, height: number, [deep, mid, tip]: THREE.Color[]): void {
  const rim = at.clone().add(v3(0.5 * radius * Math.cos(heading), 0.5 * radius * Math.sin(heading), height));
  const light = domed(rim.clone().setZ(rim.z - 2 * height), v3(1, 1, 1), 0.3);
  const point = (k: number) => {
    const a = heading + (k / 8) * 2 * Math.PI;
    const out = k % 2 ? 0.7 : 1;
    return rim.clone().add(v3(out * radius * Math.cos(a), out * radius * Math.sin(a), (out - 0.75) * height));
  };
  const [spike, notch] = [mid.clone().lerp(tip, 0.15), deep.clone().lerp(mid, 0.3)];
  for (let k = 0; k < 8; k++) cards.add([at, point(k), point(k + 1)], [deep, k % 2 ? notch : spike, k % 2 ? spike : notch], light);
}

/** Stiff sword leaves up and out from one point, only the outer ring drooping. */
function rosette(cards: Sheet, at: THREE.Vector3, size: number, count: number, [deep, mid, tip]: THREE.Color[], rand: () => number): void {
  const light = domed(at, v3(1, 1, 1), 0.3);
  for (let j = 0; j < count; j++) {
    const a = j * GOLDEN + 0.5 * rand();
    const t = (j + 0.5) / count;
    const polar = 1.3 * Math.sqrt(t) + 0.45 * smooth(0.7, 1, t);
    const reach = size * (0.8 + 0.35 * rand());
    const end = at.clone().add(v3(Math.sin(polar) * Math.cos(a), Math.sin(polar) * Math.sin(a), Math.cos(polar)).multiplyScalar(reach));
    const side = v3(-Math.sin(a), Math.cos(a), 0).multiplyScalar(0.14 * reach);
    cards.add([at.clone().sub(side), at.clone().add(side), end], [deep, deep, mid.clone().lerp(tip, smooth(0.5, 0, polar) + 0.3 * rand())], light);
  }
}

/** Squashes a lump of radius r into a pad with a flat top and round sides. */
function pad(geo: THREE.BufferGeometry, r: number): void {
  const pos = geo.getAttribute('position');
  for (let i = 0; i < pos.count; i++) pos.setZ(i, pos.getZ(i) - 0.4 * r * smooth(0.2 * r, r, pos.getZ(i)));
  geo.computeVertexNormals();
}

/** Draws the underside of a lump of radius r down into a hanging point. */
function hang(geo: THREE.BufferGeometry, r: number, amount: number): void {
  const pos = geo.getAttribute('position');
  for (let i = 0; i < pos.count; i++) {
    const k = smooth(0.2, -1, pos.getZ(i) / r);
    const narrow = 1 - 0.5 * amount * k;
    pos.setXYZ(i, pos.getX(i) * narrow, pos.getY(i) * narrow, pos.getZ(i) - amount * r * k * k);
  }
  geo.computeVertexNormals();
}

/** Shears a tree downwind, towards +x, more the higher it reaches. */
function sweep(geo: THREE.BufferGeometry, amount: number): void {
  const pos = geo.getAttribute('position');
  for (let i = 0; i < pos.count; i++) pos.setX(i, pos.getX(i) + amount * pos.getZ(i) ** 2);
  geo.computeVertexNormals();
}
