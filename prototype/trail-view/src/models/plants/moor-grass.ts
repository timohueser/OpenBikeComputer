import * as THREE from 'three';
import type { Plant } from '../library';
import { domed, GOLDEN, ramp, random, sheet, smooth, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Purple moor grass and deer grass: the straw of dead blades, tawny to orange-brown in autumn. */
const MOOR_GRASS: Record<Season, Ramp> = {
  winter: ['#6a4e32', '#a8845a', '#d4b88a'],
  spring: ['#5e4a30', '#9a7e56', '#c8ac7e'],
  summer: ['#5a4e30', '#968458', '#d2bc88'],
  autumn: ['#6a3a1e', '#a8683a', '#c99a5a'],
};

const MOOR_GREEN: Ramp = ['#3a4a1f', '#7a8c3e', '#a2ac58'];

/** Share of a tussock's blades still green, counted from the crown out. */
const MOOR_LIVE: Record<Season, number> = { winter: 0, spring: 0.3, summer: 0.95, autumn: 0.12 };

/**
 * A tussock of purple moor grass or deer grass, lit as one dome: blades taper from inside it to
 * points on a ragged top, and a fringe of long blades rises out of its edge and bows over nearly
 * to the ground. Live blades grow from the crown out as the season has them, dead straw beyond. 98
 * triangles.
 */
function moorGrass(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(MOOR_GRASS[season]);
  const [greenDeep, greenMid] = ramp(MOOR_GREEN);
  // Live blades go buff at the tip only as they turn in autumn.
  const buff = season === 'autumn' ? 0.35 : 0.1;
  const light = domed(v3(), v3(0.6, 0.6, 1), 0.3);
  const cards = sheet();
  // Root, blade and tip: a live blade is green, a dead one straw throughout. The root's shade
  // fades out by a third of the height, so a long front blade is no dark stake.
  const colours = (live: boolean, foot: number) => {
    const [r1, r2] = [rand(), rand()];
    const body = live ? greenMid.clone().lerp(tip, 0.5 * buff + 0.15 * r1) : mid.clone().lerp(tip, 0.35 + 0.4 * r1);
    const end = live ? greenMid.clone().lerp(tip, buff + 0.35 * buff * r2 + 0.15 * r2) : body.clone().lerp(tip, 0.3);
    const root = (live ? greenDeep.clone().lerp(greenMid, 0.2) : deep.clone().lerp(mid, 0.2)).lerp(body, 0.35 + 0.65 * smooth(0, 0.3, foot));
    return [root, body, end];
  };
  const across = (a: number, width: number) => {
    const turn = a + Math.PI / 2 + (rand() - 0.5) * 0.8;
    return v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(width * (0.8 + 0.4 * rand()));
  };
  const dome = 54;
  for (let j = 0; j < dome; j++) {
    const a = j * GOLDEN + rand() * 0.5;
    const polar = 1.25 * Math.sqrt((j + 0.5) / dome);
    const k = 0.88 + 0.22 * rand();
    const top = v3(0.6 * k * Math.sin(polar) * Math.cos(a), 0.6 * k * Math.sin(polar) * Math.sin(a), k * Math.cos(polar) ** 0.6 + 0.1 * rand());
    // A blade stands more upright than the dome's normal.
    const lean = 0.6 * polar;
    const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -0.6 - 0.25 * rand());
    foot.z = Math.max(foot.z, 0);
    const side = across(a, 0.04);
    const [root, body, end] = colours(j < MOOR_LIVE[season] * dome, foot.z);
    cards.add([foot.clone().add(side), foot.clone().sub(side), top], [root, body, end], light);
  }
  const fringe = 22;
  for (let j = 0; j < fringe; j++) {
    const a = ((j + rand() * 0.8) / fringe) * 2 * Math.PI;
    const polar = 0.9 + 0.25 * rand();
    const foot = v3(0.5 * Math.sin(polar) * Math.cos(a), 0.5 * Math.sin(polar) * Math.sin(a), 0.9 * Math.cos(polar) ** 0.6);
    const reach = 0.85 + 0.2 * rand();
    const out = a + (rand() - 0.5) * 0.3;
    const end = v3(reach * Math.cos(out), reach * Math.sin(out), 0.08 + 0.17 * rand());
    // A kite bent at its widest and bowed up there, so the blade arches out of the tussock's side
    // and over toward the ground in one curve.
    const bend = foot.clone().lerp(end, 0.45);
    bend.z += 0.07 + 0.05 * rand();
    const side = across(out, 0.035);
    const [root, body, tipColour] = colours(rand() < MOOR_LIVE[season], foot.z);
    cards.add([foot, bend.clone().add(side), bend.clone().sub(side), bend.clone().add(side), end, bend.clone().sub(side)], [root, body, body, body, tipColour, body], light);
  }
  return cards.build();
}

export default {
  name: 'Moor grass',
  latin: 'Molinia caerulea, Trichophorum cespitosum',
  build: (season, seed) => moorGrass(season, random(seed)),
  ground: { on: ['grass', 'shrub', 'wetland', 'moss'], height: [0.3, 0.6], share: 0.35, habitat: 'moor', own: true },
} satisfies Plant;
