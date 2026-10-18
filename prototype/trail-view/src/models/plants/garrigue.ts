import * as THREE from 'three';
import type { Plant } from '../library';
import { core, domed, GOLDEN, ramp, random, sheet, smooth, tufts, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Kermes oak, lentisc and rosemary: evergreen, a little duller in winter. */
const GARRIGUE: Record<Season, Ramp> = {
  winter: ['#2a3322', '#41502f', '#66744a'],
  spring: ['#26381e', '#41602c', '#7a9a48'],
  summer: ['#24331d', '#3b5229', '#6a8040'],
  autumn: ['#283420', '#3f5229', '#6c7c42'],
};

/**
 * Kermes oak, lentisc and rosemary in one evergreen cushion: five leafy tufts around dark cores,
 * a few of them lit by the sun. Each leaf is a kite from a foot inside its tuft out to the dome.
 */
function garrigue(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(GARRIGUE[season]);
  const shade = deep.clone().multiplyScalar(0.7);
  const cards = sheet();
  for (const tuft of tufts([0, 0, 0.52, 1], 4, 0.8, [0.4, 0.38, 0.6], [0.12, 0.1, 0.2], rand)) {
    const [x, y, r, h] = tuft;
    const light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
    const sun = rand() < 0.3 ? 0.6 : 0.2 * rand();
    core(cards, tuft, deep, deep, 0.72, 0.8);
    for (let j = 0; j < 16; j++) {
      const a = j * GOLDEN + rand() * 0.6;
      const polar = 1.55 * Math.sqrt((j + 0.5) / 16);
      const k = 0.92 + 0.16 * rand();
      const top = v3(x + r * k * Math.sin(polar) * Math.cos(a), y + r * k * Math.sin(polar) * Math.sin(a), h * k * Math.cos(polar) ** 0.6);
      // Leaves lie out along the dome like scales; the lowest hang down over the core.
      const lean = 1.1 * polar;
      const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -0.38 * h);
      foot.z = Math.max(foot.z, 0);
      const middle = foot.clone().lerp(top, 0.7);
      const turn = a + Math.PI / 2 + (rand() - 0.5) * 0.8;
      const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(0.22 * h * (0.8 + 0.4 * rand()));
      const leaf = mid.clone().lerp(tip, sun + 0.2 * rand()).lerp(deep, 0.4 * (1 - smooth(0.2, 0.9, top.z / h)));
      const lit = leaf.clone().lerp(tip, 0.5);
      cards.add([foot, middle.clone().add(side), top, foot, top, middle.clone().sub(side)], [shade, leaf, lit, shade, lit, leaf], light);
    }
  }
  return cards.build();
}

export default {
  name: 'Garrigue',
  latin: 'Quercus coccifera, Pistacia lentiscus, Salvia rosmarinus',
  build: (season, seed) => garrigue(season, random(seed)),
  ground: { on: ['shrub', 'grass', 'bare'], height: [0.35, 0.8], share: 0.16, own: true },
} satisfies Plant;
