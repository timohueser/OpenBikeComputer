import * as THREE from 'three';
import type { Plant } from '../library';
import { domed, GOLDEN, hex, lathe, merge, paint, ramp, random, relight, sheet, smooth, soft, tube, UP, v3, type Ramp, type Vec } from '../shape';
import type { Season } from '../trees';

/** Tabaiba leaves: pale glaucous green, yellowing as the summer drought sheds them. */
const TABAIBA: Record<Season, Ramp> = {
  winter: ['#5a7a4a', '#8eb070', '#c4d896'],
  spring: ['#5a7a4a', '#92b272', '#c8da98'],
  summer: ['#7a7a44', '#aaa864', '#d2c88a'],
  autumn: ['#5e7a4a', '#90ae6e', '#c0d292'],
};

/**
 * Sweet spurge: a compact dome of foliage, its surface broken into rosettes of narrow fleshy
 * leaves, on grey branches that show beneath it. Each rosette is a star of leaves biased to its
 * top, lit as a small lump of the dome. The summer drought sheds leaves, so the dome shrinks and
 * fewer rosettes sit on it among bare twigs. 302 triangles, 242 in summer.
 */
function tabaiba(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(TABAIBA[season]);
  const [bark, barkLit] = [hex('#57554e'), hex('#9a978a')];
  const dry = season === 'summer';
  const k = dry ? 0.85 : 1;
  const size = v3(0.5 * k, 0.5 * k, 0.56 * k);
  const cards = sheet();
  const parts: THREE.BufferGeometry[] = [];
  // The shade among the branches inside the bush, grey between the rosettes.
  const shell = soft(lathe([[0.95, 0.12], [1, 0.4], [0.75, 0.8], [0, 1]], 10, 3));
  shell.scale(0.85 * size.x, 0.85 * size.y, 0.85 * size.z);
  const inside = deep.clone().lerp(mid, 0.3).lerp(bark, 0.25);
  parts.push(paint(relight(shell, domed(v3(0, 0, 0.1), size, 0.15)), (p) => inside.clone().lerp(mid, 0.35 * smooth(0.15, 0.5, p.z))));
  for (let i = 0; i < 12; i++) {
    const a = i * GOLDEN + rand() * 0.4;
    const polar = Math.acos(1 - 0.85 * ((i + 0.5) / 12));
    const around = v3(Math.sin(polar) * Math.cos(a), Math.sin(polar) * Math.sin(a), Math.cos(polar));
    const centre = around.clone().multiply(size).setZ(0.08 + around.z * (size.z - 0.08)).addScaledVector(around, 0.02);
    // The low ones stand on branches; in summer every third has shed its leaves and is a bare twig.
    const bare = dry && i % 3 === 1;
    if (polar > 1 || bare) {
      const foot: Vec = [0.06 * Math.cos(a), 0.06 * Math.sin(a), -0.02];
      const end = bare ? centre.clone().addScaledVector(around, 0.04).addScaledVector(UP, 0.08) : centre.clone().addScaledVector(around, -0.1);
      parts.push(paint(soft(tube([foot, end.toArray()], 0.035, 0.5, 1, 3)), (p) => bark.clone().lerp(barkLit, smooth(0, 0.3, p.z))));
    }
    if (bare) continue;
    const axis = around.clone().lerp(UP, 0.3).normalize();
    const e1 = v3().crossVectors(axis, Math.abs(axis.z) > 0.9 ? v3(1, 0, 0) : UP).normalize();
    const e2 = v3().crossVectors(axis, e1);
    const r = (dry ? 0.19 : 0.26) * (0.9 + 0.2 * rand());
    const base = centre.clone().addScaledVector(axis, -0.3 * r);
    const sun = rand() < 0.4 ? 0.45 : 0.15 * rand();
    const low = 0.4 * (1 - smooth(0.15, 0.55, centre.z));
    const light = (p: THREE.Vector3) => p.clone().sub(centre).divideScalar(r).addScaledVector(around, 0.8).normalize().lerp(UP, 0.2).normalize();
    for (let j = 0; j < 9; j++) {
      const t = a + j * GOLDEN + rand() * 0.5;
      // Denser towards the rosette's top and none past 75 degrees, so from above it shows no hole.
      const tilt = 1.3 * ((j + 0.5) / 9) ** 0.8;
      const across = e1.clone().multiplyScalar(Math.cos(t)).addScaledVector(e2, Math.sin(t));
      const dir = axis.clone().multiplyScalar(Math.cos(tilt)).addScaledVector(across, Math.sin(tilt));
      const length = r * (0.85 + 0.25 * rand());
      const end = base.clone().addScaledVector(dir, length);
      end.z -= 0.15 * length * Math.sin(tilt);
      const middle = base.clone().lerp(end, 0.55);
      const side = e1.clone().multiplyScalar(-Math.sin(t)).addScaledVector(e2, Math.cos(t)).multiplyScalar(0.03 * (0.8 + 0.4 * rand()));
      const leaf = mid.clone().lerp(tip, sun + 0.25 * rand()).lerp(deep, low + 0.2 * smooth(0.7, 1.3, tilt));
      const lit = leaf.clone().lerp(tip, 0.5);
      cards.add([base, middle.clone().add(side), end, base, end, middle.clone().sub(side)], [deep, leaf, lit, deep, lit, leaf], light);
    }
  }
  parts.push(cards.build());
  return merge(parts);
}

export default {
  name: 'Tabaiba',
  latin: 'Euphorbia balsamifera',
  build: (season, seed) => tabaiba(season, random(seed)),
  ground: { on: ['shrub', 'grass', 'bare'], height: [0.6, 1.2], share: 0.12, own: true },
} satisfies Plant;
