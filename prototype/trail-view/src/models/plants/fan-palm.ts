import * as THREE from 'three';
import type { Plant } from '../library';
import { blade, GOLDEN, hash, hex, merge, paint, ramp, random, ribbon, smooth, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Palmito fans: blue grey-green, dustier at the end of the dry summer. */
const PALM: Record<Season, Ramp> = {
  winter: ['#34503f', '#5e8270', '#9cb698'],
  spring: ['#34543f', '#62876e', '#a4c09a'],
  summer: ['#3c503f', '#6a826c', '#a8b692'],
  autumn: ['#365040', '#62826c', '#a0b494'],
};

/** An indexed mesh with smooth normals from flat position and index arrays. */
function mesh(position: number[], index: number[]): THREE.BufferGeometry {
  const geo = new THREE.BufferGeometry().setAttribute('position', new THREE.Float32BufferAttribute(position, 3));
  geo.setIndex(index);
  geo.computeVertexNormals();
  return geo;
}

/**
 * A stiff fan facing `facing` from its hinge, continuing `along`: `segments` pleated segments
 * over `span` radians, split more than halfway in. Two triangles a segment.
 */
function fan(hinge: THREE.Vector3, facing: THREE.Vector3, along: THREE.Vector3, r: number, segments: number, span: number, rand: () => number) {
  const side = v3().crossVectors(facing, along).normalize();
  const up = v3().crossVectors(side, facing).normalize();
  const at = (a: number, d: number) => {
    const p = hinge.clone().addScaledVector(up, d * Math.cos(a)).addScaledVector(side, d * Math.sin(a)).addScaledVector(facing, (0.12 * d * d) / r);
    // Segment tips bow a little under their own weight.
    p.z -= (0.12 * d * d) / r;
    return p;
  };
  const points = [hinge];
  for (let i = 0; i <= segments; i++) points.push(at(-span / 2 + (i / segments) * span, 0.42 * r));
  for (let i = 0; i < segments; i++) points.push(at(-span / 2 + ((i + 0.5) / segments) * span, r * (0.9 + 0.15 * rand())));
  const index: number[] = [];
  for (let i = 0; i < segments; i++) {
    const tip = segments + 2 + i;
    index.push(0, 1 + i, tip, 0, tip, 2 + i);
  }
  return mesh(points.flatMap((p) => p.toArray()), index);
}

/**
 * Dwarf fan palm: nine stiff fans on short stalks, the inner ones upright, the outer ones held
 * out, round three pale folded spear leaves, and one dead fan hanging straw-brown against the
 * foot. Each fan is striped light along its segments and dark in the folds between. 24 triangles
 * a fan with its stalk, 249 in all.
 */
function fanPalm(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(PALM[season]);
  const [straw, strawLit, stalk] = [hex('#7a6444'), hex('#b09a6e'), hex('#76805a')];
  const parts: THREE.BufferGeometry[] = [];
  const count = 10;
  for (let k = 0; k < count; k++) {
    const a = k * GOLDEN + rand() * 0.5;
    const q = (k + 0.5) / count;
    const dead = k === count - 1;
    const tilt = dead ? 0.3 : 0.08 + 0.85 * Math.sqrt(q);
    const out = v3(Math.cos(a), Math.sin(a), 0);
    const dir = out.clone().multiplyScalar(Math.sin(tilt)).setZ(Math.cos(tilt));
    const foot = out.clone().multiplyScalar(0.04);
    const hinge = foot.clone().addScaledVector(dir, dead ? 0.38 : 0.52 * (1 - 0.3 * q) + 0.08 * rand());
    // Fans face out and up, the outer ones more up; the dead one hangs down the outside of the foot.
    const lift = dead ? 0.46 : 0.3 + 0.45 * q + 0.15 * rand();
    const facing = out.clone().multiplyScalar(Math.cos(lift)).setZ(Math.sin(lift));
    const along = dead ? out.clone().multiplyScalar(0.45).setZ(-0.9) : dir;
    const r = dead ? 0.3 : 0.36 + 0.06 * rand();
    const tone = 0.9 + 0.2 * rand();
    const [fold, body, lit] = dead ? [straw.clone().multiplyScalar(0.75), straw, strawLit] : [deep, mid, tip].map((c) => c.clone().multiplyScalar(tone));
    const leaf = fan(hinge, facing, along, r, 11, dead ? 2.4 : 3.5 + 0.4 * rand(), rand);
    parts.push(paint(leaf, (p) => {
      const d = p.distanceTo(hinge) / r;
      return d > 0.7 ? lit.clone().lerp(body, 0.4 * hash(p, k)) : d > 0.3 ? fold : body.clone().lerp(fold, 0.5);
    }));
    parts.push(paint(ribbon([foot.toArray(), hinge.toArray()], [-out.y, out.x, 0], () => 0.03, 1), dead ? straw : stalk));
  }
  // Young leaves stand folded in the middle, so the clump has no hole at its heart.
  for (let k = 0; k < 3; k++) {
    const a = (k / 3) * 2 * Math.PI + rand();
    const length = 0.6 + 0.15 * rand();
    const spear = blade([0.015 * Math.cos(a), 0.015 * Math.sin(a), 0], a, length, 0.08 + 0.12 * rand(), 0.03, (t) => 0.07 * (1 - t) ** 0.7, 2);
    parts.push(paint(spear, (p) => mid.clone().lerp(tip, 0.3 + 0.5 * smooth(0.2, length, p.z))));
  }
  return merge(parts);
}

export default {
  name: 'Dwarf fan palm',
  latin: 'Chamaerops humilis',
  build: (season, seed) => fanPalm(season, random(seed)),
  ground: { on: ['shrub', 'grass', 'bare'], height: [0.6, 1.2], share: 0.06, own: true },
} satisfies Plant;
