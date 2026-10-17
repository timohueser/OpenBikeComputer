/**
 * Landmarks: the buildings a place is known by, drawn as themselves rather than as a kit house.
 * Footprint-sized, base at the origin, z up, flat shaded in vertex colours.
 */

import * as THREE from 'three';
import { random } from './shape';

const STONE = ['#8a877e', '#7c7a72', '#969287'];
const SLIT = '#2c2a28';

/** Flat-shaded triangles from a list of [x, y, z] corners, three per face, in one colour per face. */
function faces(corners: number[][], colours: THREE.Color[]): THREE.BufferGeometry {
  const color: number[] = [];
  for (let f = 0; f < corners.length / 3; f++) for (let i = 0; i < 3; i++) color.push(...colours[f].toArray());
  const geometry = new THREE.BufferGeometry()
    .setAttribute('position', new THREE.Float32BufferAttribute(corners.flat(), 3))
    .setAttribute('color', new THREE.Float32BufferAttribute(color, 3));
  geometry.computeVertexNormals();
  return geometry;
}

/**
 * A roofless stone tower on a width × depth footprint, its wall tops broken to uneven heights,
 * with a few dark slits.
 */
export function ruin(width: number, depth: number, height: number, seed: number): THREE.BufferGeometry {
  const rand = random(seed);
  const corners: number[][] = [];
  const colours: THREE.Color[] = [];
  const [hx, hy] = [width / 2, depth / 2];
  const ring = [[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]];
  ring.forEach(([ax, ay], side) => {
    const [bx, by] = ring[(side + 1) % 4];
    const steps = Math.max(2, Math.round(Math.hypot(bx - ax, by - ay) / 1.4));
    for (let s = 0; s < steps; s++) {
      const [t0, t1] = [s / steps, (s + 1) / steps];
      const [x0, y0, x1, y1] = [ax + (bx - ax) * t0, ay + (by - ay) * t0, ax + (bx - ax) * t1, ay + (by - ay) * t1];
      const top = height * (1 - 0.3 * rand() ** 2);
      // Bottom to top in storeys, so a slit can sit in one.
      for (let z = -1; z < top; z += 3) {
        const z1 = Math.min(z + 3, top);
        const slit = z > 1 && z1 < top - 1 && s % 3 === 1 && rand() < 0.5;
        const c = new THREE.Color(slit ? SLIT : STONE[Math.floor(rand() * STONE.length)]);
        corners.push([x0, y0, z], [x1, y1, z], [x1, y1, z1], [x0, y0, z], [x1, y1, z1], [x0, y0, z1]);
        colours.push(c, c);
      }
    }
  });
  return faces(corners, colours);
}

/** A squat white lighthouse: a tapering tower, a dark lantern and a cap, centred on its footprint. */
export function lighthouse(width: number, depth: number, height: number): THREE.BufferGeometry {
  const r = Math.max(1.5, Math.min(width, depth) / 2);
  const tower = new THREE.CylinderGeometry(r * 0.8, r, height, 12, 1, true).rotateX(Math.PI / 2).translate(0, 0, height / 2);
  const gallery = new THREE.CylinderGeometry(r * 1.05, r * 1.05, 0.3, 12).rotateX(Math.PI / 2).translate(0, 0, height + 0.15);
  const lantern = new THREE.CylinderGeometry(r * 0.55, r * 0.55, 1.6, 10, 1, true).rotateX(Math.PI / 2).translate(0, 0, height + 1.1);
  const cap = new THREE.ConeGeometry(r * 0.65, 1, 10).rotateX(Math.PI / 2).translate(0, 0, height + 2.4);
  const parts: [THREE.BufferGeometry, string][] = [[tower, '#f2efe6'], [gallery, '#2f3a40'], [lantern, '#3a4a50'], [cap, '#2f3a40']];
  const corners: number[][] = [];
  const colours: THREE.Color[] = [];
  for (const [geometry, hex] of parts) {
    const flat = geometry.toNonIndexed().getAttribute('position');
    for (let i = 0; i < flat.count; i++) corners.push([flat.getX(i), flat.getY(i), flat.getZ(i)]);
    for (let f = 0; f < flat.count / 3; f++) colours.push(new THREE.Color(hex));
  }
  return faces(corners, colours);
}
