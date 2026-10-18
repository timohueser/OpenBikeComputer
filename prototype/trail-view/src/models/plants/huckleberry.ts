import * as THREE from 'three';
import type { Plant } from '../library';
import { blade, byHeight, hex, merge, paint, place, ramp, random, soft, tube, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

const HUCKLEBERRY: Record<Season, Ramp> = {
  // Bare red twigs: the leaves take the twigs' colour.
  winter: ['#5a3028', '#7a4434', '#8e5a48'],
  spring: ['#4f8a3a', '#7ab84e', '#b0dc72'],
  summer: ['#3a6a2e', '#56883a', '#80aa50'],
  autumn: ['#7a1e22', '#a83228', '#d4583a'],
};

/** Four red stems set with small leaves and, in late summer, berries. Winter leaves it bare. */
function huckleberry(season: Season, rand: () => number): THREE.BufferGeometry {
  const leaf = byHeight(ramp(HUCKLEBERRY[season]), 1);
  const [stem, berry] = [hex('#8a4434'), hex('#3c2a6a')];
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 4; k++) {
    const a = (k / 4) * 2 * Math.PI + rand() * 0.8;
    const [c, s] = [Math.cos(a), Math.sin(a)];
    const path = new THREE.CatmullRomCurve3([v3(), v3(0.12 * c, 0.12 * s, 0.4), v3(0.3 * c, 0.3 * s, 0.75 + 0.2 * rand())]);
    parts.push(paint(soft(tube(path.points.map((p) => p.toArray()), 0.025, 0.4, 3, 3)), stem));
    for (let i = 0; i < 7; i++) {
      const at = path.getPointAt(0.3 + 0.1 * i).toArray();
      parts.push(paint(blade(at, rand() * 2 * Math.PI, 0.2, 1.1, 0.2, (t) => 0.13 * Math.sin(Math.PI * t), 2), leaf));
    }
    if (season === 'summer') parts.push(paint(place(soft(new THREE.OctahedronGeometry(0.045)), { at: path.getPointAt(0.6 + 0.1 * rand()) }), berry));
  }
  return merge(parts);
}

export default {
  name: 'Huckleberry',
  latin: 'Vaccinium parvifolium',
  build: (season, seed) => huckleberry(season, random(seed)),
  ground: { on: ['tree'], height: [0.4, 0.8], share: 0.25 },
} satisfies Plant;
