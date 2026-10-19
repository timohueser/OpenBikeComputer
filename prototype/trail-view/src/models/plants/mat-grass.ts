import * as THREE from 'three';
import type { Plant } from '../library';
import { domed, GOLDEN, hex, ramp, random, sheet, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Live bristle leaves: grey-green, with a blue cast. */
const MAT_GREEN: Ramp = ['#4a6040', '#80a06c', '#b4c89c'];

/** Dead leaves and old culms, bleached nearly white by winter. */
const MAT_STRAW: Record<Season, Ramp> = {
  winter: ['#9a9074', '#cdc4a6', '#ece6d2'],
  spring: ['#948662', '#c6b994', '#e4dcc0'],
  summer: ['#968460', '#c4b48c', '#e2d6b2'],
  autumn: ['#9c8658', '#ccb886', '#eadcb2'],
};

/** Share of the leaves still green, counted from the middle of the tuft out. */
const MAT_LIVE: Record<Season, number> = { winter: 0, spring: 0.5, summer: 0.8, autumn: 0.3 };

/** The one-sided spike: dark violet in flower, straw once it has seeded. */
const SPIKE: Record<Season, string> = { winter: '#cfc6a8', spring: '#c4b896', summer: '#4e4258', autumn: '#b8a678' };

/**
 * A tuft of mat-grass: stiff bristle leaves that splay straight out of a tight base like a brush,
 * a skirt of bleached dead leaves round its foot, and thin culms that stand above it, each with a
 * slim spike. Live leaves grow from the middle out as the season has them. 104 triangles.
 */
function matGrass(season: Season, rand: () => number): THREE.BufferGeometry {
  const [greenDeep, greenMid, greenTip] = ramp(MAT_GREEN);
  const [deep, mid, tip] = ramp(MAT_STRAW[season]);
  const light = domed(v3(0, 0, 0.1), v3(0.5, 0.5, 0.7), 0.55);
  const cards = sheet();
  const leaf = (foot: THREE.Vector3, end: THREE.Vector3, width: number, live: boolean) => {
    const out = Math.atan2(end.y - foot.y, end.x - foot.x);
    const side = v3(-Math.sin(out), Math.cos(out), 0).multiplyScalar(width * (0.8 + 0.4 * rand()));
    const r = rand();
    // Every leaf rises out of the pale dead sheaths that wrap the base.
    const body = live ? greenDeep.clone().lerp(greenMid, 0.5 + 0.5 * r).lerp(greenTip, 0.3 * rand()) : mid.clone().lerp(tip, 0.2 + 0.7 * r);
    const root = deep.clone().lerp(mid, 0.6).lerp(body, 0.35);
    cards.add([foot.clone().add(side), foot.clone().sub(side), end], [root, root, body], light);
  };
  // Leaves leave the base at angles from upright to splayed, nearly straight, the upright ones longest.
  const leaves = 64;
  for (let j = 0; j < leaves; j++) {
    const a = j * GOLDEN + rand() * 0.4;
    const u = (j + 0.5) / leaves;
    const lean = 0.08 + 0.95 * Math.sqrt(u) + 0.12 * rand();
    const length = (0.72 - 0.25 * u) * (0.85 + 0.25 * rand());
    const foot = v3(0.08 * Math.sqrt(u) * Math.cos(a), 0.08 * Math.sqrt(u) * Math.sin(a), 0);
    const end = foot.clone().add(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean) - 0.08 * lean * lean).multiplyScalar(length));
    leaf(foot, end, 0.032, u < MAT_LIVE[season]);
  }
  // The skirt: dead leaves bent out over the ground round the base.
  for (let j = 0; j < 22; j++) {
    const a = ((j + rand() * 0.8) / 22) * 2 * Math.PI;
    const reach = 0.3 + 0.18 * rand();
    leaf(v3(0.05 * Math.cos(a), 0.05 * Math.sin(a), 0.04), v3(reach * Math.cos(a), reach * Math.sin(a), 0.02 + 0.06 * rand()), 0.03, false);
  }
  // Culms stand above the leaves; the spike is the top fifth, turned to one side as a comb.
  const spike = hex(SPIKE[season]);
  const stem = season === 'summer' ? greenMid.clone().lerp(greenTip, 0.3) : mid;
  const culms = season === 'spring' ? 2 : 6;
  for (let k = 0; k < culms; k++) {
    const a = k * GOLDEN * 2 + rand();
    const lean = 0.08 + 0.22 * rand();
    const axis = v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean));
    const foot = v3(0.03 * Math.cos(a), 0.03 * Math.sin(a), 0);
    const height = 0.88 + 0.12 * rand() - (k === 0 ? 0 : 0.1 * rand());
    const base = foot.clone().addScaledVector(axis, 0.78 * height);
    const top = foot.clone().addScaledVector(axis, height);
    const side = v3(-Math.sin(a), Math.cos(a), 0);
    cards.add([foot.clone().addScaledVector(side, 0.01), foot.clone().addScaledVector(side, -0.01), base], [deep, deep, stem], light);
    const comb = side.multiplyScalar(0.03);
    const bend = base.clone().lerp(top, 0.4).add(comb);
    cards.add([base, bend, top], [spike, spike, spike.clone().lerp(tip, 0.2)], light);
    cards.add([base.clone().addScaledVector(comb, -0.3), bend, top], [stem, spike, spike], light);
  }
  return cards.build();
}

export default {
  name: 'Mat-grass',
  latin: 'Nardus stricta',
  build: (season, seed) => matGrass(season, random(seed)),
  ground: { on: ['grass', 'moss'], height: [0.15, 0.3], share: 0.3, habitat: 'moor', own: true },
} satisfies Plant;
