import * as THREE from 'three';
import type { Plant } from '../library';
import { core, domed, GOLDEN, hex, merge, paint, ramp, random, sheet, smooth, soft, tufts, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Crowberry needles: dark green with fresh yellow-green shoot tips, bronzed olive-brown in winter. */
const CROWBERRY: Record<Season, Ramp> = {
  winter: ['#36301e', '#5e5434', '#86744a'],
  spring: ['#24361e', '#46663a', '#7a9048'],
  summer: ['#223a20', '#426c3a', '#8eac52'],
  autumn: ['#263620', '#465e36', '#7e8044'],
};

/** Share of shoots with fresh growth at the tip, and the black berries on the mat, by season. */
const FRESH: Record<Season, number> = { winter: 0, spring: 0.25, summer: 0.6, autumn: 0.15 };
const BERRIES: Record<Season, number> = { winter: 5, spring: 3, summer: 9, autumn: 11 };

const STEM = hex('#6a3424');
const BERRY = hex('#141418');
const SHINE = hex('#4a4e60');

/**
 * Crowberry: a low, flat mat about four times as wide as tall, six clumps of needle shoots round
 * dark cores that lie out over each other like shingles, red-brown stems that creep out from the
 * rim, and glossy black berries on top. Each shoot is one triangle from a narrow foot to a wide
 * tip. 228 triangles in summer.
 */
function crowberry(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(CROWBERRY[season]);
  const shade = deep.clone().multiplyScalar(0.8);
  const cards = sheet();
  const parts: THREE.BufferGeometry[] = [];
  const shoot = (foot: THREE.Vector3, top: THREE.Vector3, heading: number, low: number, light: (p: THREE.Vector3) => THREE.Vector3) => {
    const turn = heading + Math.PI / 2 + (rand() - 0.5) * 0.8;
    const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(0.16 * (0.8 + 0.4 * rand()));
    const fresh = rand() < FRESH[season] * (1 - low);
    const colour = mid.clone().lerp(tip, fresh ? 0.7 + 0.3 * rand() : 0.25 * rand()).lerp(deep, 0.45 * low);
    const corners = [top.clone().add(side), top.clone().sub(side)];
    for (const c of corners) c.z -= 0.05 * rand();
    cards.add([foot, ...corners], [shade.clone().lerp(STEM, 0.3 * rand()), colour, colour.clone().lerp(mid, 0.3 * rand())], light);
  };
  const clumps = tufts([0, 0, 0.8, 1], 5, 0.6, [0.85, 0.6, 0.65], [0.15, 0.15, 0.2], rand);
  const tops: THREE.Vector3[] = [];
  for (const clump of clumps) {
    const [x, y, r, h] = clump;
    const light = domed(v3(x, y, 0), v3(r, r, h), 0.4);
    core(cards, clump, deep, deep.clone().lerp(mid, 0.6), 0.5, 0.4);
    for (let j = 0; j < 22; j++) {
      const a = j * GOLDEN + rand() * 0.6;
      const s = Math.sqrt((j + 0.5) / 22);
      const polar = 1.5 * s;
      const k = 0.9 + 0.15 * rand();
      // A flat top that rolls down to the ground at the edge, so the rim hides the core.
      const top = v3(x + r * k * s * Math.cos(a), y + r * k * s * Math.sin(a), h * k * (1 - s ** 3));
      // Shoots lie out along the dome more than heather's sprays, so the mat reads as a carpet.
      const lean = 0.65 * polar;
      const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -0.55 * r * (0.8 + 0.4 * rand()));
      foot.z = Math.max(foot.z, 0);
      shoot(foot, top, a, 1 - smooth(0.2, 0.9, top.z / h), light);
      if (polar < 0.9) tops.push(top);
    }
  }
  // Red-brown stems creep out from under the rim and turn up into a shoot.
  const rim = domed(v3(), v3(1.7, 1.7, 0.6), 0.5);
  for (let j = 0; j < 10; j++) {
    const a = ((j + rand() * 0.7) / 10) * 2 * Math.PI;
    const out = v3(Math.cos(a), Math.sin(a), 0);
    const side = v3(-out.y, out.x, 0).multiplyScalar(0.025);
    const [from, bend] = [out.clone().multiplyScalar(1.0).setZ(0.15), out.clone().multiplyScalar(1.45 + 0.2 * rand()).setZ(0.04)];
    cards.add([from.clone().add(side), from.clone().sub(side), bend], [STEM.clone().multiplyScalar(0.6), STEM.clone().multiplyScalar(0.6), STEM], rim);
    shoot(bend, bend.clone().addScaledVector(out, 0.3).setZ(0.2 + 0.08 * rand()), a, 0.2, rim);
  }
  // Berries sit in the needles on the clumps' tops, a little proud of them.
  for (let b = 0; b < BERRIES[season] && tops.length; b++) {
    const at = tops.splice(Math.floor(rand() * tops.length), 1)[0];
    const ball = soft(new THREE.TetrahedronGeometry(0.1)).rotateZ(rand() * Math.PI).translate(at.x, at.y, at.z + 0.03);
    parts.push(paint(ball, (p) => BERRY.clone().lerp(SHINE, smooth(at.z, at.z + 0.12, p.z))));
  }
  parts.push(cards.build());
  return merge(parts);
}

export default {
  name: 'Crowberry',
  latin: 'Empetrum nigrum subsp. hermaphroditum',
  build: (season, seed) => crowberry(season, random(seed)),
  ground: { on: ['shrub', 'moss', 'bare'], height: [0.1, 0.25], share: 0.3, habitat: 'moor', own: true },
} satisfies Plant;
