import * as THREE from 'three';
import type { Plant } from '../library';
import { blade, byHeight, merge, paint, ramp, random, type Ramp, type Vec } from '../shape';
import type { Season } from '../trees';

const GRASS: Record<Season, Ramp> = {
  winter: ['#6e6a4c', '#9a9168', '#c2b88c'],
  spring: ['#4f7a35', '#7fae4a', '#b4d36e'],
  summer: ['#557a34', '#86a84a', '#c2c46a'],
  autumn: ['#6a7038', '#97935a', '#c4b47a'],
};

/** Nine tapering blades, 27 triangles. */
function grass(season: Season, rand: () => number): THREE.BufferGeometry {
  const colour = byHeight(ramp(GRASS[season]), 0.9);
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 9; k++) {
    const a = (k / 9) * 2 * Math.PI + rand() * 0.6;
    const base: Vec = [0.05 * Math.cos(a), 0.05 * Math.sin(a), 0];
    const geo = blade(base, a, 0.75 + 0.25 * rand(), 0.2 + 0.45 * rand(), 0.35, (t) => 0.1 * (1 - t), 2);
    parts.push(paint(geo, colour));
  }
  return merge(parts);
}

export default {
  name: 'Grass tuft',
  latin: 'Poaceae',
  build: (season, seed) => grass(season, random(seed)),
  ground: { on: ['grass', 'crop'], height: [0.25, 0.5], share: 1, habitat: 'lush' },
} satisfies Plant;
