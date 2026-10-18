import * as THREE from 'three';
import type { Plant } from '../library';
import { hex, merge, ramp, random, smooth, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Cardón stems: glaucous grey-green after the winter rains, yellower in the drought. */
const CARDON: Record<Season, Ramp> = {
  winter: ['#374e3b', '#62845e', '#a0b488'],
  spring: ['#38503c', '#668862', '#a6b88a'],
  summer: ['#465040', '#74825a', '#aaac7c'],
  autumn: ['#3a4f3c', '#68845e', '#a2b286'],
};

/** Red cyathia on the stem tips in spring, red-brown capsules in summer: colour and share. */
const CARDON_TIPS: Record<Season, [string, number]> = {
  winter: ['#a8b46c', 0],
  spring: ['#a8282a', 0.7],
  summer: ['#8a5a34', 0.4],
  autumn: ['#a8b46c', 0],
};

/**
 * A five-ribbed stem through `path` with radius `r` at each point, closed by a blunt cap rising
 * `cap` radii above its last point; a radius of 0 starts it in a point. Flat shaded, so the ribs
 * catch the light. `shade` gets the level along it: 0 at the foot and 1 at the top ring, then 1.5
 * at the cap's own corners on that ring and 2 at its tip. 10 triangles a segment, 5 for the cap
 * or a pointed foot.
 */
function stem(path: THREE.Vector3[], r: number[], cap: number, spin: number, shade: (p: THREE.Vector3, level: number) => THREE.Color) {
  const last = path.length - 1;
  const dirs = path.slice(1).map((p, i) => p.clone().sub(path[i]).normalize());
  // Each ring's frame comes down from the one above it, so a bent stem does not twist.
  let u = v3(Math.cos(spin), Math.sin(spin), 0);
  const rings: THREE.Vector3[][] = [];
  for (let i = last; i >= 0; i--) {
    const [a, b] = [dirs[Math.max(i - 1, 0)], dirs[Math.min(i, last - 1)]];
    const t = a.clone().add(b).normalize();
    u = u.projectOnPlane(t).normalize();
    const w = v3().crossVectors(t, u);
    // A ring at a bend lies in the mitre plane, stretched across the bend to keep the stem's girth.
    const bend = b.clone().sub(a).projectOnPlane(t);
    const stretch = 1 / t.dot(b) - 1;
    if (bend.lengthSq() > 1e-6) bend.normalize();
    rings[i] = [0, 1, 2, 3, 4].map((j) => {
      const off = u.clone().multiplyScalar(r[i] * Math.cos((j * 2 * Math.PI) / 5)).addScaledVector(w, r[i] * Math.sin((j * 2 * Math.PI) / 5));
      return path[i].clone().add(off).addScaledVector(bend, stretch * off.dot(bend));
    });
  }
  const apex = path[last].clone().addScaledVector(dirs[last - 1], cap * r[last]);
  const corners: THREE.Vector3[] = [];
  const levels: number[] = [];
  const push = (ps: THREE.Vector3[], ls: number[]) => ps.forEach((p, i) => (corners.push(p), levels.push(ls[i])));
  for (let i = 0; i < last; i++) {
    const [a, b] = [i / last, (i + 1) / last];
    for (let j = 0; j < 5; j++) {
      const n = (j + 1) % 5;
      if (r[i] > 0) push([rings[i][j], rings[i][n], rings[i + 1][n]], [a, a, b]);
      push([rings[i][j], rings[i + 1][n], rings[i + 1][j]], [a, b, b]);
    }
  }
  for (let j = 0; j < 5; j++) push([rings[last][j], rings[last][(j + 1) % 5], apex], [1.5, 1.5, 2]);
  const geo = new THREE.BufferGeometry()
    .setAttribute('position', new THREE.Float32BufferAttribute(corners.flatMap((p) => p.toArray()), 3))
    .setAttribute('color', new THREE.Float32BufferAttribute(corners.flatMap((p, i) => shade(p, levels[i]).toArray()), 3));
  geo.computeVertexNormals();
  return geo;
}

/**
 * Cardón: a dense candelabra mound of five-ribbed stems. Seven stand straight up in the middle;
 * ten round them leave the base sideways, narrow and low, and bend upright at an elbow. In flower
 * the tips and the tops of the stems below them turn red. 305 triangles.
 */
function cardon(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(CARDON[season]);
  const [flower, share] = CARDON_TIPS[season];
  const [wood, bloom] = [hex('#6a6450'), hex(flower)];
  const parts: THREE.BufferGeometry[] = [];
  const add = (path: THREE.Vector3[], r: number[]) => {
    const h = path[path.length - 1].z;
    const tone = 0.88 + 0.24 * rand();
    const flowering = rand() < share;
    const shade = (p: THREE.Vector3, level: number) => {
      const c = deep.clone().lerp(mid, smooth(0, 0.35, p.z)).lerp(tip, 0.55 * smooth(h - 0.3, h, p.z)).multiplyScalar(tone);
      c.lerp(wood, 0.6 * (1 - smooth(0, 0.12, p.z)));
      return flowering ? c.lerp(bloom, level > 1 ? 0.75 : 0.15 * smooth(0.99, 1, level)) : c;
    };
    parts.push(stem(path, r, 0.45, rand() * Math.PI, shade));
  };
  // A tight bundle in the middle, its stems touching: one at the centre and six around it.
  for (let k = 0; k < 7; k++) {
    const a = (k / 6) * 2 * Math.PI + rand() * 0.3;
    const out = v3(Math.cos(a), Math.sin(a), 0);
    const d = k === 6 ? 0.02 : 0.17 + 0.02 * rand();
    const r = 0.085 * (0.92 + 0.16 * rand());
    add([out.clone().multiplyScalar(0.7 * d).setZ(-0.03), out.clone().multiplyScalar(d + 0.03).setZ(1 - 0.5 * d - 0.1 * rand())], [0.85 * r, r]);
  }
  for (let k = 0; k < 10; k++) {
    const a = ((k + 0.5) / 10) * 2 * Math.PI + (rand() - 0.5) * 0.3;
    const out = v3(Math.cos(a), Math.sin(a), 0);
    const d = 0.44 + 0.08 * rand();
    const r = 0.085 * (0.92 + 0.16 * rand());
    const path = [out.clone().multiplyScalar(0.12).setZ(0), out.clone().multiplyScalar(d).setZ(0.12 + 0.05 * rand()), out.clone().multiplyScalar(d + 0.03).setZ(0.48 + 0.22 * rand())];
    add(path, [0, r, r]);
  }
  return merge(parts);
}

export default {
  name: 'Cardón',
  latin: 'Euphorbia canariensis',
  build: (season, seed) => cardon(season, random(seed)),
  ground: { on: ['shrub', 'bare'], height: [1, 2], share: 0.03, own: true },
} satisfies Plant;
