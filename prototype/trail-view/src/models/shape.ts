/**
 * Soft primitives for the near-field models. A builder paints each part with vertex colours and
 * merges the parts into one geometry with position, normal and color, so one material draws a
 * whole model. z is up, as in the terrain's east-north-up frame.
 */

import * as THREE from 'three';
import { mergeGeometries, mergeVertices } from 'three/examples/jsm/utils/BufferGeometryUtils.js';

export type Vec = [number, number, number];
export type Shade = THREE.Color | ((p: THREE.Vector3, n: THREE.Vector3) => THREE.Color);

export const v3 = (x = 0, y = 0, z = 0) => new THREE.Vector3(x, y, z);
export const hex = (h: string) => new THREE.Color(h);

export function smooth(a: number, b: number, x: number): number {
  const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
  return t * t * (3 - 2 * t);
}

/** A deterministic wobble, so hand-made lumps are the same on every load. */
export function noise(p: THREE.Vector3, f: number, s: number): number {
  return (
    Math.sin(p.x * f + s) * Math.sin(p.y * f * 1.31 + s * 2.1) * Math.sin(p.z * f * 0.87 + s * 0.7) +
    0.5 * Math.sin(p.x * f * 2.13 - s) * Math.sin(p.z * f * 1.71 + s * 1.3)
  );
}

/**
 * A steady random value in [0, 1) per point. Dots finer than a model's vertex spacing come out
 * as single dappled vertices, where smooth noise would make stripes.
 */
export function hash(p: THREE.Vector3, seed: number): number {
  const h = Math.sin(p.x * 127.1 + p.y * 311.7 + p.z * 74.7 + seed * 17.3) * 43758.5453;
  return h - Math.floor(h);
}

/** Seeded random numbers in [0, 1), so scattered details are the same on every load. */
export function random(seed = 1): () => number {
  let s = (seed * 7919 + 1) % 2147483647 || 1;
  return () => (s = (s * 16807) % 2147483647) / 2147483647;
}

/**
 * Welds seams so normals stay smooth, then pushes the surface along its normals. Triangles that
 * weld to a line, like a lathe's rows at the axis, are dropped.
 */
export function soft(geometry: THREE.BufferGeometry, amp = 0, freq = 3, seed = 0): THREE.BufferGeometry {
  geometry.deleteAttribute('uv');
  geometry.deleteAttribute('normal');
  const geo = mergeVertices(geometry);
  const index = geo.index!.array;
  const kept: number[] = [];
  for (let i = 0; i < index.length; i += 3) {
    const [a, b, c] = [index[i], index[i + 1], index[i + 2]];
    if (a !== b && b !== c && a !== c) kept.push(a, b, c);
  }
  geo.setIndex(kept);
  geo.computeVertexNormals();
  if (amp) {
    const pos = geo.getAttribute('position');
    const nrm = geo.getAttribute('normal');
    const p = v3();
    const n = v3();
    for (let i = 0; i < pos.count; i++) {
      p.fromBufferAttribute(pos, i);
      n.fromBufferAttribute(nrm, i);
      p.addScaledVector(n, amp * noise(p, freq, seed));
      pos.setXYZ(i, p.x, p.y, p.z);
    }
    geo.computeVertexNormals();
  }
  return geo;
}

/** Colours from a constant or from (position, normal) in the part's own frame. */
export function paint(geo: THREE.BufferGeometry, shade: Shade): THREE.BufferGeometry {
  const colour = typeof shade === 'function' ? shade : () => shade;
  const pos = geo.getAttribute('position');
  const nrm = geo.getAttribute('normal');
  const out = new Float32Array(pos.count * 3);
  const p = v3();
  const n = v3();
  for (let i = 0; i < pos.count; i++) colour(p.fromBufferAttribute(pos, i), n.fromBufferAttribute(nrm, i)).toArray(out, i * 3);
  geo.setAttribute('color', new THREE.BufferAttribute(out, 3));
  return geo;
}

interface Placement {
  at?: Vec | THREE.Vector3;
  rot?: Vec;
  scale?: Vec;
  quat?: THREE.Quaternion;
}

export function place(geo: THREE.BufferGeometry, { at = [0, 0, 0], rot = [0, 0, 0], scale = [1, 1, 1], quat }: Placement = {}) {
  const q = quat ?? new THREE.Quaternion().setFromEuler(new THREE.Euler(...rot));
  const p = at instanceof THREE.Vector3 ? at : v3(...at);
  return geo.applyMatrix4(new THREE.Matrix4().compose(p, q, v3(...scale)));
}

export const ball = (r: number, w = 24, h = 16) => new THREE.SphereGeometry(r, w, h);

/** A round lump without poles: 20 triangles at detail 0, 80 at 1, 180 at 2, 320 at 3. */
export const lump = (r: number, detail: number) => new THREE.IcosahedronGeometry(r, detail);

/**
 * A surface of revolution around z. The profile runs from the bottom of the axis outwards and up,
 * so faces point outwards; `rows` samples its spline.
 */
export function lathe(profile: [number, number][], segments = 32, rows = profile.length * 4): THREE.BufferGeometry {
  const curve = new THREE.SplineCurve(profile.map(([x, z]) => new THREE.Vector2(x, z)));
  const pts = curve.getPoints(rows);
  for (const p of pts) p.x = Math.max(0, p.x);
  return new THREE.LatheGeometry(pts, segments).rotateX(Math.PI / 2);
}

/** A capsule from a to b, painted in the model's frame. */
export function limb(a: Vec, b: Vec, r: number, shade: Shade, amp = 0.01, sides = 12): THREE.BufferGeometry {
  const dir = v3(...b).sub(v3(...a));
  const geo = place(soft(new THREE.CapsuleGeometry(r, dir.length(), Math.max(1, sides >> 2), sides), amp, 8), {
    at: v3(...a).add(v3(...b)).multiplyScalar(0.5),
    quat: new THREE.Quaternion().setFromUnitVectors(v3(0, 1, 0), dir.normalize()),
  });
  return paint(geo, shade);
}

/** A tube along a smooth path, its radius shrinking linearly to `tip` times `r` at the end. */
export function tube(points: Vec[], r: number, tip = 1, segments = 16, sides = 8): THREE.BufferGeometry {
  const path = new THREE.CatmullRomCurve3(points.map((p) => v3(...p)));
  const geo = new THREE.TubeGeometry(path, segments, r, sides);
  const pos = geo.getAttribute('position');
  const c = v3();
  const p = v3();
  for (let i = 0; i <= segments; i++) {
    path.getPointAt(i / segments, c);
    const k = 1 + (tip - 1) * (i / segments);
    for (let j = 0; j <= sides; j++) {
      const idx = i * (sides + 1) + j;
      p.fromBufferAttribute(pos, idx).sub(c).multiplyScalar(k).add(c);
      pos.setXYZ(idx, p.x, p.y, p.z);
    }
  }
  return geo;
}

/**
 * A flat strip along a smooth path, `width(t)` wide across `side`, for blades and fronds. Its
 * normals lean `lift` of the way to up, so thin plants light like the ground they stand on.
 */
export function ribbon(points: Vec[], side: Vec, width: (t: number) => number, segments: number, lift = 0.5) {
  const path = new THREE.CatmullRomCurve3(points.map((p) => v3(...p)));
  const across = v3(...side).normalize();
  const position: number[] = [];
  const c = v3();
  for (let i = 0; i <= segments; i++) {
    const t = i / segments;
    path.getPointAt(t, c);
    const w = width(t) / 2;
    position.push(c.x - across.x * w, c.y - across.y * w, c.z - across.z * w, c.x + across.x * w, c.y + across.y * w, c.z + across.z * w);
  }
  const index: number[] = [];
  for (let i = 0; i < segments; i++) index.push(2 * i, 2 * i + 1, 2 * i + 3, 2 * i, 2 * i + 3, 2 * i + 2);
  const geo = new THREE.BufferGeometry().setAttribute('position', new THREE.Float32BufferAttribute(position, 3));
  geo.setIndex(index);
  const strip = soft(geo);
  const nrm = strip.getAttribute('normal');
  const n = v3();
  for (let i = 0; i < nrm.count; i++) {
    n.fromBufferAttribute(nrm, i);
    if (n.z < 0) n.negate();
    n.lerp(v3(0, 0, 1), lift).normalize();
    nrm.setXYZ(i, n.x, n.y, n.z);
  }
  return strip;
}

/** Scales a model uniformly so its top is at z = 1. */
export function unit(geo: THREE.BufferGeometry): THREE.BufferGeometry {
  geo.computeBoundingBox();
  const s = 1 / geo.boundingBox!.max.z;
  return geo.scale(s, s, s);
}

export function merge(parts: THREE.BufferGeometry[]): THREE.BufferGeometry {
  const geo = mergeGeometries(parts);
  if (!geo) throw new Error('parts differ in their attributes');
  return geo;
}

export function triangles(geo: THREE.BufferGeometry): number {
  return (geo.index ? geo.index.count : geo.getAttribute('position').count) / 3;
}
