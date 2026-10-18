import * as THREE from 'three';
import type { Plant } from '../library';
import { blade, byHeight, hex, merge, paint, place, ramp, random, ribbon, smooth, soft, type Ramp, type Vec } from '../shape';
import type { Season } from '../trees';

/** Leaves in the grass tuft's summer green. */
const LEAF: Ramp = ['#557a34', '#86a84a', '#c2c46a'];

/** Gentian, buttercup, edelweiss white, aster and pink, as the season has them. */
const FLOWERS: Record<Season, string[]> = {
  winter: ['#8a7456', '#9a8462'],
  spring: ['#2f5fd0', '#f2c832', '#f6f2e6', '#2f5fd0'],
  summer: ['#2f5fd0', '#f2c832', '#f6f2e6', '#9a6ad0', '#e48ab0'],
  autumn: ['#9a6ad0', '#8a7456', '#a08a5a'],
};

/** A flat five-petalled star facing up, its eye in the middle: ten triangles. */
function star(r: number, petal: THREE.Color, eye: THREE.Color): THREE.BufferGeometry {
  const geo = new THREE.CircleGeometry(r, 10);
  const pos = geo.getAttribute('position');
  for (let i = 2; i < pos.count; i += 2) pos.setXYZ(i, pos.getX(i) * 0.5, pos.getY(i) * 0.5, 0);
  return paint(soft(geo), (p) => eye.clone().lerp(petal, smooth(0.1 * r, 0.5 * r, Math.hypot(p.x, p.y))));
}

/** A low rosette with seven flowers on stalks. */
function flowers(season: Season, rand: () => number): THREE.BufferGeometry {
  const leaf = byHeight(ramp(LEAF), 0.4);
  const stalk = hex(season === 'winter' || season === 'autumn' ? '#8a7a52' : '#5c8a45');
  const colours = FLOWERS[season];
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 5; k++) {
    const a = (k / 5) * 2 * Math.PI + rand();
    parts.push(paint(blade([0, 0, 0], a, 0.45, 1.1, 0.35, (t) => 0.18 * Math.sin(Math.PI * Math.min(1, t + 0.15)), 2), leaf));
  }
  for (let k = 0; k < 7; k++) {
    const a = (k / 7) * 2 * Math.PI + rand() * 0.8;
    const d = 0.15 + 0.3 * rand();
    const h = 0.45 + 0.55 * rand();
    const top: Vec = [d * Math.cos(a), d * Math.sin(a), h];
    const middle: Vec = [0.4 * top[0], 0.4 * top[1], 0.55 * h];
    parts.push(paint(ribbon([[0, 0, 0], middle, top], [-Math.sin(a), Math.cos(a), 0], () => 0.03, 2), stalk));
    const petal = hex(colours[Math.floor(rand() * colours.length)]);
    const head = star(0.14, petal, petal.clone().lerp(hex('#f6e27a'), 0.7));
    parts.push(place(head, { at: top, rot: [(rand() - 0.5) * 0.6, (rand() - 0.5) * 0.6, rand() * 6] }));
  }
  return merge(parts);
}

export default {
  name: 'Alpine flowers',
  latin: 'Gentiana, Ranunculus, Leontopodium, Aster, Dianthus',
  build: (season, seed) => flowers(season, random(seed)),
  ground: { on: ['grass'], height: [0.2, 0.35], share: 0.15 },
} satisfies Plant;
