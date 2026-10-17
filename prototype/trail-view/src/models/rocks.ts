/**
 * Rocks, 1 tall with their base at the origin and z up: faceted lumps sunk into the ground, flat
 * shaded so each face catches the light on its own, as low-poly rock reads in a game.
 */

import * as THREE from 'three';
import { hash, hex, lump, noise, smooth, unit } from './shape';

/** Grey body, a lit top, and lichen: yellow-green and the orange of salt-sprayed rock. */
const BODY = ['#7d7f78', '#8e908a', '#a3a49c'].map(hex);
const LICHEN = ['#b4b05a', '#c98a3a'].map(hex);

/** A low slab, wider than tall; a rounder boulder; or a crag, a leaning blade taller than wide. About 80 faces. */
function build(name: RockName, seed: number): THREE.BufferGeometry {
  const slab = name === 'slab';
  const geo = lump(1, 1).toNonIndexed();
  const pos = geo.getAttribute('position');
  const p = new THREE.Vector3();
  for (let i = 0; i < pos.count; i++) {
    p.fromBufferAttribute(pos, i);
    p.multiplyScalar(1 + 0.22 * noise(p, 2.2, seed));
    // Flat underneath, so it sits in the ground instead of on a point.
    p.z = Math.max(p.z, -0.35) * (slab ? 0.42 : name === 'crag' ? 1.9 : 0.8);
    if (slab) p.x *= 1.35;
    if (name === 'crag') [p.x, p.y] = [p.x * 0.55 + 0.25 * p.z, p.y * 0.9];
    pos.setXYZ(i, p.x, p.y, p.z);
  }
  // A fifth of it below the ground.
  geo.computeBoundingBox();
  const { min, max } = geo.boundingBox!;
  geo.translate(0, 0, -min.z - 0.2 * (max.z - min.z));
  geo.computeVertexNormals();
  const normal = geo.getAttribute('normal');
  const colour = new Float32Array(pos.count * 3);
  const n = new THREE.Vector3();
  for (let i = 0; i < pos.count; i += 3) {
    // One colour per face, so the facets stay crisp.
    p.fromBufferAttribute(pos, i);
    n.fromBufferAttribute(normal, i);
    const c = BODY[0].clone().lerp(BODY[1], smooth(-0.4, 0.4, n.z)).lerp(BODY[2], smooth(0.6, 0.95, n.z));
    const fleck = hash(p, seed);
    if (n.z > 0.3 && fleck < 0.18) c.lerp(LICHEN[fleck < 0.05 ? 1 : 0], 0.35);
    c.multiplyScalar(0.9 + 0.2 * hash(p, seed + 1));
    for (let j = 0; j < 3; j++) c.toArray(colour, (i + j) * 3);
  }
  geo.setAttribute('color', new THREE.BufferAttribute(colour, 3));
  return unit(geo);
}

export type RockName = 'slab' | 'boulder' | 'crag';

export const rock = (name: RockName, seed = 1) => build(name, seed);
