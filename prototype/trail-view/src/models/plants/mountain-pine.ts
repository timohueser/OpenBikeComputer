import * as THREE from 'three';
import type { Plant } from '../library';
import { core, domed, GOLDEN, hex, merge, paint, ramp, random, sheet, smooth, soft, tube, tufts, v3, type Ramp, type Vec } from '../shape';
import type { Season } from '../trees';

/** Mountain pine needles: dark green, a little olive in winter. */
const MUGO: Record<Season, Ramp> = {
  winter: ['#1c3024', '#2e4c34', '#5a7a4c'],
  spring: ['#1a3224', '#2c5236', '#62904e'],
  summer: ['#1a3024', '#2c4e36', '#5a884c'],
  autumn: ['#1a3024', '#2c4c34', '#58844a'],
};

/** Pale new shoots on the top brushes: colour and share, by season. */
const MUGO_CANDLES: Record<Season, [string, number]> = {
  winter: ['#4c6a42', 0],
  spring: ['#a8b878', 0.5],
  summer: ['#7a9a5a', 0.3],
  autumn: ['#4c6a42', 0],
};

/**
 * Mountain pine: six round clumps of upturned needle brushes around dark cores, a low mound about
 * twice as wide as tall, and two crooked limbs that creep out from under it along the ground and
 * turn up into brushes of their own. Each brush is one triangle from a narrow foot to a wide top,
 * standing more upright than the dome it sits on; all light as their clump does.
 */
function mountainPine(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(MUGO[season]);
  const [candle, share] = MUGO_CANDLES[season];
  const shoot = hex(candle);
  const [bark, barkLit] = [hex('#3a302a'), hex('#6e6052')];
  const shade = deep.clone().multiplyScalar(0.75);
  const cards = sheet();
  const parts: THREE.BufferGeometry[] = [];
  // A brush faces out along `heading`; `low` darkens the ones deep in the clump.
  const brush = (foot: THREE.Vector3, top: THREE.Vector3, heading: number, low: number, light: (p: THREE.Vector3) => THREE.Vector3) => {
    const turn = heading + Math.PI / 2 + (rand() - 0.5) * 0.8;
    const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(0.09 * (0.8 + 0.4 * rand()));
    const colour = mid.clone().lerp(tip, 0.15 + 0.45 * rand()).lerp(deep, 0.4 * low);
    if (low < 0.25 && rand() < share) colour.lerp(shoot, 0.6);
    const corners = [top.clone().add(side), top.clone().sub(side)];
    for (const c of corners) c.z -= 0.04 * rand();
    cards.add([foot, ...corners], [shade.clone().lerp(mid, 0.2 * rand()), colour, colour.clone().lerp(deep, 0.25)], light);
  };
  const clumps = tufts([0, 0, 0.5, 0.9], 5, 0.7, [0.48, 0.42, 0.55], [0.1, 0.08, 0.2], rand);
  for (const clump of clumps) {
    const [x, y, r, h] = clump;
    const light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
    core(cards, clump, shade, deep.clone().lerp(mid, 0.3), 0.75, 0.8);
    for (let j = 0; j < 35; j++) {
      const a = j * GOLDEN + rand() * 0.6;
      const polar = 1.5 * Math.sqrt((j + 0.5) / 35);
      const k = 0.9 + 0.15 * rand();
      const top = v3(x + r * k * Math.sin(polar) * Math.cos(a), y + r * k * Math.sin(polar) * Math.sin(a), h * k * Math.cos(polar) ** 0.5);
      // Brushes turn up at the branch ends, so they stand more upright than the dome.
      const lean = 0.35 * polar;
      const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -h * (0.28 + 0.12 * rand()));
      foot.z = Math.max(foot.z, 0);
      brush(foot, top, a, 1 - smooth(0.2, 0.9, top.z / h), light);
    }
  }
  for (const [x, y, r] of [clumps[1], clumps[3]]) {
    const d = Math.hypot(x, y);
    const out = v3(x / d, y / d, 0);
    const bend = v3(-out.y, out.x, 0).multiplyScalar((rand() - 0.5) * 0.3);
    const end = out.clone().multiplyScalar(d + 0.9 * r).setZ(0.2);
    const path: Vec[] = [[0.2 * out.x, 0.2 * out.y, 0.03], out.clone().multiplyScalar(0.7 * d).add(bend).setZ(0.04).toArray(), out.clone().multiplyScalar(d + 0.8 * r).add(bend).setZ(0.05).toArray(), end.toArray()];
    parts.push(paint(soft(tube(path, 0.04, 0.6, 3, 3)), (p) => bark.clone().lerp(barkLit, smooth(0, 0.3, p.z))));
    const light = domed(end.clone().setZ(0), v3(0.2, 0.2, 0.45), 0.3);
    for (let i = 0; i < 5; i++) {
      const a = (i / 5) * 2 * Math.PI + rand() * 0.6;
      brush(end, end.clone().add(v3(0.08 * Math.cos(a), 0.08 * Math.sin(a), 0.18 + 0.06 * rand())).addScaledVector(out, 0.05), a, 0, light);
    }
  }
  parts.push(cards.build());
  return merge(parts);
}

export default {
  name: 'Mountain pine',
  latin: 'Pinus mugo',
  build: (season, seed) => mountainPine(season, random(seed)),
  ground: { on: ['shrub'], height: [1, 2.5], share: 0.04, own: true },
} satisfies Plant;
