import * as THREE from 'three';
import type { Plant } from '../library';
import { domed, GOLDEN, hex, ramp, random, sheet, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Reed stems and leaves: blue-green in summer, gold in autumn, straw in winter. */
const REED: Record<Season, Ramp> = {
  winter: ['#7a6440', '#b09868', '#d8c49a'],
  spring: ['#4a5e2a', '#78984a', '#a8c06e'],
  summer: ['#3a5630', '#62844a', '#94b070'],
  autumn: ['#7a6a30', '#a89848', '#d0bc6a'],
};

/** Plumes by season: the colour at the stem, the colour at the edge, and their size. */
const PLUMES: Record<Season, [string, string, number]> = {
  winter: ['#a89a84', '#e6ded0', 1.1],
  spring: ['#8a7a64', '#c8b89c', 0.8],
  summer: ['#5a4442', '#8e766c', 0.8],
  autumn: ['#8a7058', '#dccab0', 1.1],
};

/** Winter keeps one leaf a stem, gone orange-brown. */
const WINTER_LEAF = hex('#a87444');

/**
 * A stand of common reed: 18 upright stems, each with leaves that leave it at an angle and droop
 * at the tip, and a plume that nods at the top. In spring every other stem is a short green shoot
 * among last year's straw. 270 triangles in summer and autumn.
 */
function commonReed(season: Season, rand: () => number): THREE.BufferGeometry {
  const green = ramp(REED[season]);
  const straw = ramp(REED.winter);
  const [inner, edge, size] = PLUMES[season];
  const [plumeIn, plumeOut] = [hex(inner), hex(edge)];
  const light = domed(v3(), v3(0.3, 0.3, 1), 0.4);
  const cards = sheet();
  const stems = 18;
  for (let k = 0; k < stems; k++) {
    const shoot = season === 'spring' && k % 2 === 1;
    const [deep, mid, tip] = season === 'spring' && !shoot ? straw : green;
    const a = k * GOLDEN + rand() * 0.5;
    const r = 0.22 * Math.sqrt((k + 0.5) / stems);
    const out = v3(Math.cos(a), Math.sin(a), 0);
    const side = v3(-out.y, out.x, 0);
    const foot = out.clone().multiplyScalar(r);
    const length = (shoot ? 0.5 : 0.8) + 0.2 * rand();
    const lean = 0.03 + 0.3 * r + 0.05 * rand();
    const axis = out.clone().multiplyScalar(Math.sin(lean)).setZ(Math.cos(lean));
    const top = foot.clone().addScaledVector(axis, length);
    // Thicker than a real stem, as the stand has far fewer stems than a real bed.
    const thick = side.clone().multiplyScalar(0.011);
    cards.add([foot.clone().add(thick), foot.clone().sub(thick), top], [deep, deep, mid], light);
    // Alternate leaves up the stem; winter has shed all but one.
    const leaves = season === 'winter' ? 1 : 4;
    for (let i = 0; i < leaves; i++) {
      const node = foot.clone().addScaledVector(axis, length * (0.22 + 0.17 * i + 0.06 * rand()));
      const b = a + Math.PI * i + (rand() - 0.5) * 1.2;
      const dir = v3(Math.cos(b), Math.sin(b), 0);
      const across = v3(-dir.y, dir.x, 0).multiplyScalar(0.018 * (0.8 + 0.4 * rand()));
      const [l, tilt] = [0.18 + 0.08 * rand(), 0.5 + 0.4 * rand()];
      const bend = node.clone().addScaledVector(dir, 0.45 * l * Math.sin(tilt)).setZ(node.z + 0.45 * l * Math.cos(tilt));
      const end = node.clone().addScaledVector(dir, 1.1 * l * Math.sin(tilt)).setZ(node.z + l * (Math.cos(tilt) - 0.45));
      const [body, ends] = season === 'winter' ? [WINTER_LEAF, WINTER_LEAF.clone().lerp(tip, 0.3)] : [mid.clone().lerp(tip, 0.3 * rand()), tip];
      cards.add([node, bend.clone().add(across), bend.clone().sub(across)], [deep.clone().lerp(body, 0.5), body, body], light);
      cards.add([bend.clone().add(across), end, bend.clone().sub(across)], [body, ends, body], light);
    }
    if (shoot) continue;
    // The plume nods out, away from the stand: three slim kites crossed round its axis.
    const nod = 0.25 + 0.6 * rand();
    const hang = axis.clone().multiplyScalar(Math.cos(nod)).addScaledVector(out, Math.sin(nod)).normalize();
    const reach = 0.18 * size * (0.8 + 0.4 * rand());
    const [widest, point] = [top.clone().addScaledVector(hang, 0.35 * reach), top.clone().addScaledVector(hang, reach)];
    for (let j = 0; j < 3; j++) {
      const half = side.clone().applyAxisAngle(hang, (j * Math.PI) / 3).multiplyScalar(0.03 * size);
      cards.add([top, widest.clone().add(half), point], [plumeIn, plumeOut, plumeOut], light);
      cards.add([top, point, widest.clone().sub(half)], [plumeIn, plumeOut, plumeOut], light);
    }
  }
  return cards.build();
}

export default {
  name: 'Common reed',
  latin: 'Phragmites australis',
  build: (season, seed) => commonReed(season, random(seed)),
  ground: { on: ['wetland'], height: [1.5, 3], share: 0.5, habitat: 'lush', own: true, clumped: true },
} satisfies Plant;
