import * as THREE from 'three';
import type { Plant } from '../library';
import { core, domed, GOLDEN, hash, hex, ramp, random, sheet, smooth, tufts, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

const HEATHER: Record<Season, Ramp> = {
  winter: ['#3a3a26', '#5a5434', '#7a6a40'],
  spring: ['#34462b', '#5b7340', '#7d9150'],
  summer: ['#34462b', '#5b7340', '#86984f'],
  autumn: ['#3a4228', '#5f6838', '#8c6b40'],
};

/** Flowers in bloom on a clump of bell heather and ling, by season: colour and share. */
const HEATHER_BLOOM: Record<Season, [string, number]> = {
  winter: ['#7a5a3c', 0.3],
  spring: ['#7d9150', 0],
  summer: ['#9a5a8c', 0.8],
  autumn: ['#8a5a62', 0.5],
};

/**
 * Five round tufts of upright twig sprays around a dark core. Each spray is one triangle from a
 * narrow foot to a ragged top on its tuft's dome. Flowers, or out of bloom rusty tips, colour the
 * top corners, the highest most.
 */
export function heather(season: Season, bloom: boolean, seed: number, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(HEATHER[season]);
  const [flower, share] = bloom ? HEATHER_BLOOM[season] : ['#7d6a44', 0.15];
  const [dots, bronze, shade] = [hex(flower), hex('#6a5530'), deep.clone().multiplyScalar(0.7)];
  const cards = sheet();
  for (const tuft of tufts([0, 0, 0.52, 1], 4, 0.8, [0.4, 0.38, 0.6], [0.12, 0.1, 0.2], rand)) {
    const [x, y, r, h] = tuft;
    const light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
    core(cards, tuft, shade, deep, 0.75, 0.7);
    for (let j = 0; j < 40; j++) {
      const a = j * GOLDEN + rand() * 0.6;
      const polar = 1.5 * Math.sqrt((j + 0.5) / 40);
      const k = 0.9 + 0.15 * rand();
      const top = v3(x + r * k * Math.sin(polar) * Math.cos(a), y + r * k * Math.sin(polar) * Math.sin(a), h * k * Math.cos(polar) ** 0.4);
      // A spray leans out with the dome, but stands more upright than the dome's normal.
      const lean = 0.3 * polar;
      const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -h * (0.45 + 0.2 * rand()));
      foot.z = Math.max(foot.z, 0);
      const turn = a + Math.PI / 2 + (rand() - 0.5) * 0.8;
      const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(0.075 * h * (0.8 + 0.4 * rand()));
      const twig = mid
        .clone()
        .lerp(tip, 0.2 + 0.3 * rand())
        .lerp(bronze, rand() < 0.35 ? 0.5 * rand() : 0)
        .lerp(deep, 0.35 * (1 - smooth(0.2, 0.9, top.z / h)));
      const corners = [top.clone().add(side), top.clone().sub(side)];
      for (const c of corners) c.z -= 0.08 * h * rand();
      const tips = corners.map((c) => (hash(c, seed) < share ? twig.clone().lerp(dots, (bloom ? 1 : 0.6) * (0.2 + 0.8 * smooth(0.4, 0.95, c.z / h))) : twig));
      cards.add([foot, ...corners], [shade.clone().lerp(mid, 0.2 * rand()), ...tips], light);
    }
  }
  return cards.build();
}

export default {
  name: 'Heather',
  latin: 'Calluna vulgaris',
  build: (season, seed) => heather(season, false, seed, random(seed)),
  ground: { on: ['shrub', 'moss', 'bare'], height: [0.2, 0.4], share: 0.4, habitat: 'moor' },
} satisfies Plant;
