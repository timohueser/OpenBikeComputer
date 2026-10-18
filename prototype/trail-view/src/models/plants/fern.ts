import * as THREE from 'three';
import type { Plant } from '../library';
import { blade, byHeight, merge, paint, ramp, random, smooth, type Ramp } from '../shape';
import type { Season } from '../trees';

const FERN: Record<Season, Ramp> = {
  winter: ['#5a4030', '#7a5638', '#94704a'],
  spring: ['#4f8a3a', '#7ab84e', '#b0dc72'],
  summer: ['#2f5e2a', '#4a8236', '#7aab4c'],
  autumn: ['#7a4a22', '#a8682c', '#cf9448'],
};

/** Nine fronds rising out of the crown like a shuttlecock, their tips nodding; the edge steps in and out like the pinnae. */
function fern(season: Season, rand: () => number): THREE.BufferGeometry {
  const colour = byHeight(ramp(FERN[season]), 0.7);
  const width = (t: number) => 0.45 * smooth(0.02, 0.35, t) * (1 - t) ** 0.6 * (Math.round(t * 6) % 2 ? 0.6 : 1);
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 9; k++) {
    const a = (k / 9) * 2 * Math.PI + rand() * 0.5;
    parts.push(paint(blade([0, 0, 0], a, 1.1 + 0.3 * rand(), 0.3 + 0.35 * rand(), 0.5, width, 6), colour));
  }
  return merge(parts);
}

export default {
  name: 'Fern',
  latin: 'Dryopteris filix-mas',
  build: (season, seed) => fern(season, random(seed)),
  ground: { on: ['tree'], height: [0.5, 0.9], share: 0.3 },
} satisfies Plant;
