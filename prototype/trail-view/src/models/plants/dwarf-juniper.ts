import * as THREE from 'three';
import type { Plant } from '../library';
import { core, domed, GOLDEN, hash, hex, ramp, random, sheet, smooth, tufts, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Dwarf juniper needles: grey-green, a little bronzed in winter. */
const JUNIPER: Record<Season, Ramp> = {
  winter: ['#33362c', '#5a604e', '#909682'],
  spring: ['#2a3e32', '#56725a', '#98b294'],
  summer: ['#2a3e32', '#54705a', '#92ae92'],
  autumn: ['#2c3c32', '#526a56', '#8ca48a'],
};

/** The pale band on each needle, which greys the mat where the light catches the shoot tips. */
const BLOOM = hex('#b4c4bc');
const BERRY = hex('#3a4458');

/**
 * A dwarf juniper: a low mat about three and a half times as wide as tall, six flat clumps of
 * needle shoots around dark cores, and shoots that creep out along the ground from its rim. Each
 * shoot is one triangle from a narrow foot to a wide tip; those on the rim lie outward over the
 * ground. 204 triangles.
 */
function dwarfJuniper(season: Season, seed: number, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(JUNIPER[season]);
  const shade = deep.clone().multiplyScalar(0.85);
  const cards = sheet();
  // A shoot faces out along `heading`; `low` darkens the ones deep in the mat.
  const shoot = (foot: THREE.Vector3, top: THREE.Vector3, heading: number, low: number, light: (p: THREE.Vector3) => THREE.Vector3) => {
    const turn = heading + Math.PI / 2 + (rand() - 0.5) * 0.8;
    const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(0.16 * (0.8 + 0.4 * rand()));
    const colour = mid.clone().lerp(tip, 0.1 + 0.5 * rand()).lerp(deep, 0.35 * low);
    const corners = [top.clone().add(side), top.clone().sub(side)];
    for (const c of corners) c.z -= 0.06 * rand();
    const tips = corners.map((c) => {
      const h = hash(c, seed);
      return h < 0.04 ? BERRY : colour.clone().lerp(BLOOM, h < 0.3 ? 0.3 * (1 - low) : 0);
    });
    cards.add([foot, ...corners], [shade.clone().lerp(mid, 0.2 * rand()), ...tips], light);
  };
  const clumps = tufts([0, 0, 0.9, 1], 5, 0.6, [0.85, 0.7, 0.6], [0.1, 0.15, 0.25], rand);
  for (const clump of clumps) {
    const [x, y, r, h] = clump;
    const light = domed(v3(x, y, 0), v3(r, r, h), 0.4);
    core(cards, clump, shade, deep.clone().lerp(mid, 0.5), 0.65, 0.5);
    for (let j = 0; j < 26; j++) {
      const a = j * GOLDEN + rand() * 0.6;
      const polar = 1.5 * Math.sqrt((j + 0.5) / 26);
      const k = 0.9 + 0.15 * rand();
      // A flat top that falls away to the ground at the clump's edge.
      const top = v3(x + r * k * Math.sin(polar) * Math.cos(a), y + r * k * Math.sin(polar) * Math.sin(a), h * k * Math.cos(polar) ** 0.45);
      const lean = 0.7 * polar;
      const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -0.5 * r * (0.8 + 0.4 * rand()));
      foot.z = Math.max(foot.z, 0);
      shoot(foot, top, a, 1 - smooth(0.2, 0.9, top.z / h), light);
    }
  }
  // Shoots creeping out from under the rim, their tips just off the ground.
  const rim = domed(v3(), v3(1.6, 1.6, 0.6), 0.5);
  for (let j = 0; j < 12; j++) {
    const a = ((j + rand() * 0.7) / 12) * 2 * Math.PI;
    const out = v3(Math.cos(a), Math.sin(a), 0);
    shoot(out.clone().multiplyScalar(1.1).setZ(0.25), out.clone().multiplyScalar(1.55 + 0.25 * rand()).setZ(0.08 + 0.08 * rand()), a, 0.3, rim);
  }
  return cards.build();
}

export default {
  name: 'Dwarf juniper',
  latin: 'Juniperus communis subsp. nana',
  build: (season, seed) => dwarfJuniper(season, seed, random(seed)),
  ground: { on: ['shrub', 'grass', 'bare', 'moss'], height: [0.3, 0.6], share: 0.08, habitat: 'moor', own: true, clumped: true },
} satisfies Plant;
