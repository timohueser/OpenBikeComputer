import * as THREE from 'three';
import type { Plant } from '../library';
import { blade, GOLDEN, merge, paint, ramp, random, smooth, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Esparto blades: grey-green at the root, pale straw at the tip. */
const ESPARTO: Record<Season, Ramp> = {
  winter: ['#66654a', '#9a9672', '#c4b98e'],
  spring: ['#5f6a40', '#98a06a', '#d2c88e'],
  summer: ['#6b6a45', '#a39d6a', '#d8c98e'],
  autumn: ['#6b6a45', '#a39d6a', '#d4c58e'],
};

/** Esparto: a tussock of long wiry blades arching out of the crown, grey-green below, straw at the tips. */
function esparto(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(ESPARTO[season]);
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 26; k++) {
    const a = k * GOLDEN + rand() * 0.4;
    // Inner blades stand, outer ones bow out; no tip reaches the ground.
    const tilt = 0.1 + 0.8 * Math.sqrt(k / 26) * (0.8 + 0.2 * rand());
    const arch = Math.min(0.7 * tilt * (0.8 + 0.4 * rand()), Math.cos(tilt) - 0.1);
    const ends = tip.clone().lerp(mid, 0.4 * rand());
    const colour = (p: THREE.Vector3) => deep.clone().lerp(mid, smooth(0, 0.35, p.length())).lerp(ends, smooth(0.4, 0.95, p.length()));
    const d = 0.04 + 0.08 * Math.sqrt(k / 26);
    parts.push(paint(blade([d * Math.cos(a), d * Math.sin(a), 0], a, 0.9 + 0.4 * rand(), tilt, arch, (t) => 0.06 * (1 - t), 2), colour));
  }
  return merge(parts);
}

export default {
  name: 'Esparto',
  latin: 'Macrochloa tenacissima',
  build: (season, seed) => esparto(season, random(seed)),
  ground: { on: ['grass', 'bare', 'shrub'], height: [0.4, 0.8], share: 0.4, own: true },
} satisfies Plant;
