/**
 * Aerial cableways in metres, z up, the cable along +y: pylons, stations and the carriers that hang
 * from the cable. Parts are boxes, bars, tubes and lofts in one colour each; creased normals over
 * the whole model shade tubes and rounded cabins smooth and keep box edges crisp.
 *
 * One seed per line gives its pylons, stations and carriers the same steel and livery; a gondola
 * line takes a seed per cabin, so its cabins come in many colours.
 */

import * as THREE from 'three';
import { toCreasedNormals } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { hex, merge, tube, v3, type Vec } from './shape';

export type AerialwayKind = 'cable_car' | 'gondola' | 'chair_lift' | 'drag_lift';

/**
 * Each cable's distance from the line's centre. A T-bar's riders pass beside the mast, so its
 * cables stand wider apart than its small sheaves need.
 */
export const TRACK_HALF_M: Record<AerialwayKind, number> = { cable_car: 4, gondola: 2.7, chair_lift: 2.2, drag_lift: 1.2 };

/**
 * A station's cables leave at x = ±TRACK_HALF_M, y = 0 at this height over its ground and run
 * along +y. A cable car's cabins board a storey up, clear of the slope in front; a chair's seat
 * comes down to 0.6 m over the ground, a wound-up T-bar to grabbing height.
 */
export const CABLE_AT_STATION_M: Record<AerialwayKind, number> = { cable_car: 8.4, gondola: 6, chair_lift: 4.2, drag_lift: 4.5 };

/** Station footings reach this far below their ground, so a slope does not show under them. */
const FOOTING_M = 3;

const CREASE = THREE.MathUtils.degToRad(50);

/** Galvanised, reseda green and blue-grey paint. */
const STEEL = ['#a3aaab', '#86a08c', '#8c979f'].map(hex);
const LIVERY = ['#d0402f', '#2f6db0', '#e8b530', '#3f9a5c', '#e5702d'].map(hex);
const TRAM = ['#c8372d', '#2f5f9e'].map(hex);
const DARK = hex('#30363a');
const CONCRETE = hex('#b8b2a6');
const RENDER = hex('#e6dfcf');
const GLASS = hex('#3b4c58');
/** A hall's big panes, lighter than a cabin's windows: they hold the sky. */
const PANES = hex('#7790a0');
const WHITE = hex('#eeece6');
const TIMBER = hex('#8f6440');
const SLATE = hex('#5a5652');

/** Consecutive seeds take different colours, so five cabins in a row show all five liveries. */
const pick = (list: THREE.Color[], seed: number, salt: number) => list[Math.abs(Math.floor(seed) + salt) % list.length];
const shade = (c: THREE.Color, k: number) => c.clone().multiplyScalar(k);

/**
 * A part in one colour, or one colour per group (a box's faces: +x, -x, +y, -y, +z, -z), as
 * non-indexed triangles without uv and normals. Triangles without area are dropped.
 */
function part(geo: THREE.BufferGeometry, colour: THREE.Color | THREE.Color[]): THREE.BufferGeometry {
  const flat = geo.index ? geo.toNonIndexed() : geo;
  const pos = flat.getAttribute('position');
  const colours = Array.isArray(colour) ? colour : [colour];
  const groupAt = (i: number) => flat.groups.find((g) => i >= g.start && i < g.start + g.count)?.materialIndex ?? 0;
  const position: number[] = [];
  const color: number[] = [];
  const [a, b, c] = [v3(), v3(), v3()];
  for (let i = 0; i < pos.count; i += 3) {
    a.fromBufferAttribute(pos, i);
    b.fromBufferAttribute(pos, i + 1);
    c.fromBufferAttribute(pos, i + 2);
    if (b.clone().sub(a).cross(c.clone().sub(a)).lengthSq() < 1e-10) continue;
    const k = colours[groupAt(i) % colours.length];
    for (const p of [a, b, c]) position.push(p.x, p.y, p.z), color.push(k.r, k.g, k.b);
  }
  return new THREE.BufferGeometry()
    .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
    .setAttribute('color', new THREE.Float32BufferAttribute(color, 3));
}

const finish = (parts: THREE.BufferGeometry[]) => toCreasedNormals(merge(parts), CREASE);

function box([x, y, z]: Vec, [sx, sy, sz]: Vec, colour: THREE.Color | THREE.Color[]): THREE.BufferGeometry {
  return part(new THREE.BoxGeometry(sx, sy, sz).translate(x, y, z), colour);
}

/**
 * A square bar from a to b, `w` across: w[1] lies along y for a bar in the x–z plane and along x
 * for a bar along y. Open at its ends unless capped.
 */
function beam(a: Vec, b: Vec, w: number | [number, number], colour: THREE.Color, capped = false): THREE.BufferGeometry {
  const [w0, w1] = typeof w === 'number' ? [w, w] : w;
  const from = v3(...a);
  const d = v3(...b).sub(from);
  const length = d.length();
  d.normalize();
  const across = v3().crossVectors(d, Math.abs(d.y) < 0.95 ? v3(0, 1, 0) : v3(1, 0, 0)).normalize();
  const geo = capped ? new THREE.BoxGeometry(1, 1, 1) : new THREE.CylinderGeometry(Math.SQRT1_2, Math.SQRT1_2, 1, 4, 1, true).rotateY(Math.PI / 4);
  const frame = new THREE.Matrix4().makeBasis(across, d, v3().crossVectors(across, d)).setPosition(from.addScaledVector(d, length / 2));
  return part(geo.scale(w0, length, w1).applyMatrix4(frame), colour);
}

/** A flat bar of width w from a to b facing `out`: lattice bracing, the angle iron in a tower's face. */
function strip(a: Vec, b: Vec, w: number, out: THREE.Vector3, colour: THREE.Color): THREE.BufferGeometry {
  const [p, q] = [v3(...a), v3(...b)];
  const across = v3().crossVectors(q.clone().sub(p), out).normalize().multiplyScalar(w / 2);
  const c = [p.clone().sub(across), p.clone().add(across), q.clone().add(across), q.clone().sub(across)];
  const geo = new THREE.BufferGeometry().setFromPoints([c[0], c[1], c[2], c[0], c[2], c[3]]);
  return part(geo, colour);
}

/** A shape in the x–z plane drawn `length` along y, centred on y = 0. */
function extrude(points: [number, number][], length: number, colour: THREE.Color): THREE.BufferGeometry {
  const shape = new THREE.Shape(points.map(([x, z]) => new THREE.Vector2(x, z)));
  const geo = new THREE.ExtrudeGeometry(shape, { depth: length, bevelEnabled: false, curveSegments: 1 });
  return part(geo.rotateX(Math.PI / 2).translate(0, length / 2, 0), colour);
}

/**
 * A gable roof on walls `width` × `length` whose tops are at z, the ridge along y, overhanging by
 * `eave`, with its gable ends filled in the wall colour.
 */
function roof([x, y, z]: Vec, width: number, length: number, pitch: number, eave: number, colour: THREE.Color, wall: THREE.Color) {
  const k = Math.tan(THREE.MathUtils.degToRad(pitch));
  const [h, o, t] = [width / 2, eave, 0.32];
  const ridge = h * k;
  const slab = extrude([[-h - o, -o * k], [0, ridge], [h + o, -o * k], [h + o, t - o * k], [0, ridge + t], [-h - o, t - o * k]], length + 2 * o, colour);
  const attic = extrude([[-h, 0], [h, 0], [0, ridge]], length, wall);
  return [slab, attic].map((g) => g.translate(x, y, z));
}

/**
 * A rounded rectangle, counter-clockwise from above, its corners in `steps` facets of radius r,
 * cut along its x sides at `cuts` (y values) so a band can paint narrow pillars there.
 */
function outline(hx: number, hy: number, r: number, steps: number, cuts: number[] = []): THREE.Vector2[] {
  const points: THREE.Vector2[] = [];
  const inner = cuts.filter((y) => Math.abs(y) < hy - r);
  [[1, 1], [-1, 1], [-1, -1], [1, -1]].forEach(([sx, sy], k) => {
    for (let i = 0; i <= steps; i++) {
      const a = ((k + i / steps) * Math.PI) / 2;
      points.push(new THREE.Vector2(sx * (hx - r) + r * Math.cos(a), sy * (hy - r) + r * Math.sin(a)));
    }
    // After the second corner the -x side runs down y, after the fourth the +x side runs up.
    if (k === 1) for (const y of [...inner].sort((p, q) => q - p)) points.push(new THREE.Vector2(-hx, y));
    if (k === 3) for (const y of [...inner].sort((p, q) => p - q)) points.push(new THREE.Vector2(hx, y));
  });
  return points;
}

/**
 * An outline swept up through rings of [z, scale], capped at both ends. `side` paints each band's
 * faces from the band's index, the face's outward normal and its middle in the outline.
 */
function loft(
  foot: THREE.Vector2[],
  rings: [number, number][],
  side: (band: number, n: THREE.Vector2, m: THREE.Vector2) => THREE.Color,
  bottom: THREE.Color,
  top: THREE.Color,
): THREE.BufferGeometry {
  const position: number[] = [];
  const color: number[] = [];
  const tri = (c: THREE.Color, ...ps: THREE.Vector3[]) => {
    for (const p of ps) position.push(p.x, p.y, p.z), color.push(c.r, c.g, c.b);
  };
  const at = (i: number, [z, s]: [number, number]) => v3(foot[i % foot.length].x * s, foot[i % foot.length].y * s, z);
  rings.slice(1).forEach((ring, band) => {
    const low = rings[band];
    for (let i = 0; i < foot.length; i++) {
      const [a, b] = [foot[i], foot[(i + 1) % foot.length]];
      const n = new THREE.Vector2(b.y - a.y, a.x - b.x).normalize();
      const c = side(band, n, a.clone().add(b).multiplyScalar(0.5));
      tri(c, at(i, low), at(i + 1, low), at(i + 1, ring));
      tri(c, at(i, low), at(i + 1, ring), at(i, ring));
    }
  });
  const [first, last] = [rings[0], rings[rings.length - 1]];
  for (let i = 1; i + 1 < foot.length; i++) {
    tri(bottom, at(0, first), at(i + 1, first), at(i, first));
    tri(top, at(0, last), at(i, last), at(i + 1, last));
  }
  return new THREE.BufferGeometry()
    .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
    .setAttribute('color', new THREE.Float32BufferAttribute(color, 3));
}

// Pylons.

/**
 * A cable car's tapering lattice tower: four legs, X-braced faces in panels that shorten as the
 * tower narrows, and a broad cross-arm with a rope saddle at each end. 200 to 320 triangles.
 */
function lattice(height: number, steel: THREE.Color): THREE.BufferGeometry[] {
  const T = TRACK_HALF_M.cable_car;
  const head = height - 0.9;
  const top = [1.1, 0.8];
  const base = [top[0] + 0.065 * head, top[1] + 0.05 * head];
  const n = Math.min(8, Math.max(3, Math.round(head / 6)));
  const q = Math.pow(top[0] / base[0], 1 / n);
  const zs = Array.from({ length: n + 1 }, (_, i) => ((1 - q ** i) / (1 - q ** n)) * head);
  const SQUARE = [[1, 1], [-1, 1], [-1, -1], [1, -1]];
  const corner = (z: number, k: number): Vec => {
    const t = z / head;
    const [sx, sy] = SQUARE[k % 4];
    return [sx * THREE.MathUtils.lerp(base[0], top[0], t), sy * THREE.MathUtils.lerp(base[1], top[1], t), z];
  };
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 4; k++) {
    const [x, y] = corner(0, k);
    parts.push(beam(corner(-0.3, k), corner(head, k), 0.42, steel), box([x, y, 0.25], [1.1, 1.1, 0.9], CONCRETE));
    const [a, b] = [SQUARE[k], SQUARE[(k + 1) % 4]];
    const out = v3(a[0] + b[0], a[1] + b[1], 0).normalize();
    for (let i = 0; i < n; i++) {
      parts.push(strip(corner(zs[i], k), corner(zs[i + 1], k + 1), 0.2, out, steel), strip(corner(zs[i], k + 1), corner(zs[i + 1], k), 0.2, out, steel));
      if (i + 1 < n) parts.push(strip(corner(zs[i + 1], k), corner(zs[i + 1], k + 1), 0.2, out, steel));
    }
  }
  // The arm is deepest at the tower: braces under its ends would stand where the cabins pass.
  const [end, root] = [T + 0.3, top[0] + 0.2];
  parts.push(
    box([0, 0, head - 0.4], [2 * top[0] + 0.3, 2 * top[1] + 0.3, 0.8], shade(steel, 0.8)),
    extrude([[-end, -0.5], [-root, -2.2], [root, -2.2], [end, -0.5], [end, 0], [-end, 0]], 0.9, steel).translate(0, 0, height - 0.4),
    ...[-1, 1].map((s) => box([s * T, 0, height - 0.2], [0.55, 2.8, 0.4], DARK)),
  );
  return parts;
}

/** Mast radius at the foot and head, sheaves per side and their radius. */
const MAST: Record<Exclude<AerialwayKind, 'cable_car'>, { r: [number, number]; wheels: number; wheel: number }> = {
  gondola: { r: [0.55, 0.38], wheels: 4, wheel: 0.26 },
  chair_lift: { r: [0.42, 0.3], wheels: 4, wheel: 0.21 },
  drag_lift: { r: [0.24, 0.17], wheels: 2, wheel: 0.16 },
};

/**
 * A tubular single mast with a T cross-arm; at each end a sheave train under the cable. The
 * carriers' hangers bend out past the sheaves, so the arm stops inside the tracks. 228 triangles
 * with two sheaves a side, 356 with four.
 */
function mast(kind: Exclude<AerialwayKind, 'cable_car'>, height: number, steel: THREE.Color): THREE.BufferGeometry[] {
  const T = TRACK_HALF_M[kind];
  const { r: [r0, r1], wheels, wheel } = MAST[kind];
  const axle = height - wheel;
  const arm = axle - wheel - 0.3;
  const dark = shade(steel, 0.8);
  const parts = [
    box([0, 0, 0.15], [4 * r0 + 0.5, 4 * r0 + 0.5, 0.7], CONCRETE),
    part(new THREE.CylinderGeometry(r1, r0, arm, 8, 1, true).rotateX(Math.PI / 2).translate(0, 0, arm / 2), steel),
    box([0, 0, arm], [2.4 * r1, 2.4 * r1, 0.8], dark),
    box([0, 0, arm], [2 * (T - 0.14), 1.6 * r1, 0.4], steel),
  ];
  const pitch = 2 * wheel + 0.08;
  for (const s of [-1, 1]) {
    const x = s * (T - 0.15);
    parts.push(box([x, 0, axle], [0.14, wheels * pitch, 0.18], dark), box([x, 0, (arm + axle) / 2], [0.16, 0.22, axle - arm], dark));
    for (let k = 0; k < wheels; k++) {
      const y = (k - (wheels - 1) / 2) * pitch;
      parts.push(part(new THREE.CylinderGeometry(wheel, wheel, 0.14, 8).rotateZ(Math.PI / 2).translate(s * T, y, axle), DARK));
    }
  }
  return parts;
}

/**
 * A pylon with its base at the origin; the cables run along +y over its top at z = height and
 * x = ±TRACK_HALF_M[kind]. A cable car stands on a lattice tower, the others on a tubular mast.
 */
export function pylon(kind: AerialwayKind, height: number, seed = 1): THREE.BufferGeometry {
  const steel = pick(STEEL, seed, 0);
  return finish(kind === 'cable_car' ? lattice(height, steel) : mast(kind, height, steel));
}

// Stations.

/** An operator's hut on a concrete plinth: timber walls, a band of windows all round and a gable roof. */
function hut([x, y]: [number, number], livery: THREE.Color): THREE.BufferGeometry[] {
  const [w, l, h] = [2.6, 3.2, 2.7];
  return [
    box([x, y, (0.3 - FOOTING_M) / 2], [w + 0.3, l + 0.3, 0.3 + FOOTING_M], CONCRETE),
    box([x, y, 0.3 + h / 2], [w, l, h], TIMBER),
    box([x, y, 2.05], [w + 0.08, l + 0.08, 0.9], GLASS),
    ...roof([x, y, 0.3 + h], w, l, 30, 0.45, livery, TIMBER),
  ];
}

/**
 * A cable car's terminal: a rendered hall on a concrete storey, its roof in the line's livery rising
 * toward the line. At +y it opens a storey up in a bay for each cabin, 7 m deep over a platform at
 * the cabin's floor, so a cabin can stand in it with its middle at y = 4.5; glazing fills the high
 * front over the bays.
 */
function tramStation(livery: THREE.Color): THREE.BufferGeometry[] {
  const T = TRACK_HALF_M.cable_car;
  const cable = CABLE_AT_STATION_M.cable_car;
  const [hx, hy, back, nose] = [T + 3, 8, 1, 2];
  const [floor, bay, low, high] = [cable - 5.4, cable + 0.8, cable, cable + 3.6];
  const [depth, mid] = [hy - back, (hy + back) / 2];
  const rise = (high - low) / (2 * hy);
  const eave = (y: number) => low + (y + hy) * rise;
  const dim = shade(RENDER, 0.35);
  // A profile in the y–z plane, drawn `width` across x.
  const side = (points: [number, number][], width: number, colour: THREE.Color) => extrude(points, width, colour).rotateZ(Math.PI / 2);
  const [o, front, t] = [0.8, 1.8, 0.45];
  return [
    box([0, 0, (floor - FOOTING_M) / 2], [2 * hx + 0.3, 2 * hy + 0.3, floor + FOOTING_M], CONCRETE),
    box([0, hy + nose / 2, (floor - FOOTING_M) / 2], [2 * T + 3.4, nose, floor + FOOTING_M], CONCRETE),
    side([[-hy, floor], [back, floor], [back, eave(back)], [-hy, low]], 2 * hx, RENDER),
    box([0, back + 0.03, (floor + bay) / 2], [2 * hx - 1.4, 0.06, bay - floor], dim),
    box([-hx + 0.35, mid, (floor + bay) / 2], [0.7, depth, bay - floor], [dim, RENDER, RENDER, RENDER, RENDER, RENDER]),
    box([hx - 0.35, mid, (floor + bay) / 2], [0.7, depth, bay - floor], [RENDER, dim, RENDER, RENDER, RENDER, RENDER]),
    box([0, mid, (floor + bay) / 2], [1, depth, bay - floor], dim),
    side([[back, bay], [hy, bay], [hy, high], [back, eave(back)]], 2 * hx, RENDER),
    box([0, mid, bay - 0.05], [2 * hx - 1.4, depth, 0.1], dim),
    box([0, hy + 0.04, (bay + high) / 2 - 0.25], [2 * hx - 1, 0.1, high - bay - 1.5], PANES),
    ...[-1, 1].map((s) => box([s * (hx + 0.03), -1, floor + 2.6], [0.1, 2 * hy - 5, 1.4], GLASS)),
    box([-3, -hy - 0.1, 1.2], [3.2, 0.2, 2.4], GLASS),
    side([[-hy - 0.1, low - 0.8], [hy + 0.1, high - 0.8], [hy + 0.1, high], [-hy - 0.1, low]], 2 * hx + 0.2, SLATE),
    side([[-hy - o, low - o * rise], [hy + front, high + front * rise], [hy + front, high + front * rise + t], [-hy - o, low - o * rise + t]], 2 * hx + 1.2, livery),
  ];
}

/**
 * A gondola terminal: a concrete base up to the boarding floor, a glazed hall on corner columns
 * under a broad roof in the line's livery. The cabins leave at +y through an open bay under the
 * roof, in front of the hall's end wall.
 */
function gondolaStation(livery: THREE.Color, steel: THREE.Color): THREE.BufferGeometry[] {
  const T = TRACK_HALF_M.gondola;
  const cable = CABLE_AT_STATION_M.gondola;
  const [hx, hy, floor, eave, bay] = [T + 2, 7, cable - 3.8, cable + 1, 2];
  const [wx, y0, y1] = [hx - 0.2, 0.2 - hy, hy - 0.2 - bay];
  const parts = [
    box([0, 0, (floor - FOOTING_M) / 2], [2 * hx, 2 * hy, floor + FOOTING_M], CONCRETE),
    box([0, (y0 + y1) / 2, (floor + eave) / 2], [2 * wx, y1 - y0, eave - floor], [PANES, PANES, shade(CONCRETE, 0.5), PANES, PANES, PANES]),
    ...roof([0, 0, eave], 2 * hx, 2 * hy, 14, 0.9, livery, shade(livery, 0.8)),
    extrude([[-4.5, 0], [0, 0], [0, floor]], 1.6, CONCRETE).rotateZ(-Math.PI / 2).translate(-hx - 0.8, -hy + 0.4, 0),
  ];
  for (const sx of [-1, 1]) {
    for (const sy of [-1, 1]) parts.push(box([sx * (hx - 0.25), sy * (hy - 0.25), (floor + eave) / 2], [0.45, 0.45, eave - floor], steel));
    for (let i = 1; i < 5; i++) {
      const y = y0 + ((y1 - y0) * i) / 5;
      parts.push(strip([sx * (wx + 0.05), y, floor], [sx * (wx + 0.05), y, eave], 0.16, v3(sx, 0, 0), steel));
    }
    parts.push(strip([sx * (wx + 0.05), y0, floor + 1.1], [sx * (wx + 0.05), y1, floor + 1.1], 0.16, v3(sx, 0, 0), steel));
  }
  return parts;
}

/**
 * A chair lift's or T-bar's terminal: the cable turns round the rim of a bullwheel lying flat, its
 * spokes and rim left bare round a narrow hood over the hub and drive. An A-frame behind the wheel
 * carries the hood, clear of the carriers that swing round under it; the operator's hut stands
 * beside.
 */
function liftStation(kind: 'chair_lift' | 'drag_lift', livery: THREE.Color, steel: THREE.Color): THREE.BufferGeometry[] {
  const T = TRACK_HALF_M[kind];
  const z = CABLE_AT_STATION_M[kind];
  const chairs = kind === 'chair_lift';
  // A carrier swinging round reaches this far beyond the rim: half a chair, half a T.
  const rear = -(T + (chairs ? 1.25 : 0.55) + 0.6);
  const [hx, y0, y1, spread] = chairs ? [1.4, rear - 0.4, 1.2, 1.9] : [0.85, rear - 0.4, 0.75, 1.3];
  const hood = loft(
    outline(hx, (y1 - y0) / 2, 0.6 * hx, 2),
    [[z + 0.3, 1], [z + 0.45, 1], [z + 1.0, 1], [z + 1.2, 0.86]],
    (band) => (band === 0 ? WHITE : livery),
    shade(livery, 0.5),
    livery,
  ).translate(0, (y0 + y1) / 2, 0);
  const legAt = (h: number) => spread * (1 - (0.45 * (h + FOOTING_M)) / (z + FOOTING_M));
  const parts = [
    hood,
    part(new THREE.TorusGeometry(T, 0.17, 4, 16).translate(0, 0, z), DARK),
    part(new THREE.CylinderGeometry(0.35, 0.35, 0.7, 8).rotateX(Math.PI / 2).translate(0, 0, z + 0.05), DARK),
    ...[1, 3, 5, 7].map((k) => {
      const [c, s] = [Math.cos((k * Math.PI) / 4), Math.sin((k * Math.PI) / 4)];
      return beam([0.3 * c, 0.3 * s, z], [(T - 0.1) * c, (T - 0.1) * s, z], 0.14, DARK);
    }),
    box([0, rear, z + 0.1], [2 * legAt(z) + 0.6, 0.6, 0.5], steel),
    ...hut([T + (chairs ? 3.2 : 2.4), 1.2], livery),
  ];
  for (const s of [-1, 1]) {
    parts.push(
      beam([s * spread, rear, -FOOTING_M], [s * legAt(z), rear, z], 0.42, steel, true),
      box([s * spread, rear, (0.35 - FOOTING_M) / 2], [0.9, 0.9, 0.35 + FOOTING_M], CONCRETE),
    );
  }
  return parts;
}

/**
 * A station with its origin at the ground in its middle. Its cables leave at x = ±TRACK_HALF_M,
 * y = 0, z = CABLE_AT_STATION_M along +y, from inside the hall or off the bullwheel.
 */
export function station(kind: AerialwayKind, seed = 1): THREE.BufferGeometry {
  const livery = pick(kind === 'cable_car' ? TRAM : LIVERY, seed, 1);
  const steel = pick(STEEL, seed, 0);
  if (kind === 'cable_car') return finish(tramStation(livery));
  if (kind === 'gondola') return finish(gondolaStation(livery, steel));
  return finish(liftStation(kind, livery, steel));
}

// Carriers.

/** How far below the cable a chair's seat is. */
const CHAIR_SEAT_M = 3.6;

/**
 * A large cabin, 6 × 3 × 3 m, the long side along the cable: a tapered skirt, big windows leaning
 * out between corner and side pillars, a white roof, and a C-shaped hanger from the carriage.
 */
function tram(seed: number): THREE.BufferGeometry[] {
  const body = pick(TRAM, seed, 1);
  const steel = pick(STEEL, seed, 0);
  const foot = outline(1.5, 3, 0.55, 1, [-1.08, -0.92, 0.92, 1.08]);
  const rings: [number, number][] = [[-5.3, 0.86], [-4.85, 1], [-4.1, 1], [-2.95, 1.04], [-2.55, 1.04], [-2.35, 0.97]];
  const glazed = (n: THREE.Vector2, m: THREE.Vector2) => Math.abs(n.y) > 0.99 || (Math.abs(n.x) > 0.99 && Math.abs(Math.abs(m.y) - 1) > 0.1);
  const side = (band: number, n: THREE.Vector2, m: THREE.Vector2) => [shade(body, 0.72), body, glazed(n, m) ? GLASS : body, body, WHITE][band];
  return [
    loft(foot, rings, side, DARK, WHITE),
    box([0, 0, 0.15], [0.7, 3.6, 0.6], shade(steel, 0.55)),
    beam([0.1, 0, -0.05], [0.95, 0, -0.6], [0.22, 0.8], steel, true),
    beam([0.95, 0, -0.5], [0.95, 0, -1.85], [0.22, 0.8], steel, true),
    beam([0.95, 0, -1.75], [0, 0, -2.4], [0.22, 0.8], steel, true),
  ];
}

/** A rounded 8-seat cabin, 1.8 × 2.2 × 2.2 m, windows all round between coloured corners. */
function gondola(seed: number): THREE.BufferGeometry[] {
  const body = pick(LIVERY, seed, 1);
  const steel = pick(STEEL, seed, 0);
  const rings: [number, number][] = [[-3.7, 0.8], [-3.4, 1], [-2.8, 1], [-1.8, 1.03], [-1.65, 1.02], [-1.5, 0.85]];
  const side = (band: number, n: THREE.Vector2) => [shade(body, 0.72), body, Math.abs(n.x) > 0.99 || Math.abs(n.y) > 0.99 ? GLASS : body, body, WHITE][band];
  return [
    loft(outline(0.9, 1.1, 0.4, 2), rings, side, DARK, WHITE),
    box([0, 0, 0], [0.24, 0.5, 0.3], DARK),
    part(tube([[0.04, 0, -0.06], [0.42, 0, -0.28], [0.56, 0, -0.8], [0.34, 0, -1.35], [0, 0, -1.53]], 0.055, 1, 6, 6), steel),
  ];
}

/** A 4-seat chair facing +y with padded seat and backrest, its safety bar down over a footrest. */
function chair(seed: number): THREE.BufferGeometry[] {
  const pad = pick(LIVERY, seed, 1);
  const steel = pick(STEEL, seed, 0);
  const [top, back, seat, half] = [-CHAIR_SEAT_M + 0.65, -0.32, -CHAIR_SEAT_M, 1.22];
  return [
    box([0, 0, 0], [0.2, 0.45, 0.26], DARK),
    part(tube([[0.04, 0, -0.06], [0.4, 0, -0.3], [0.48, 0, -1.1], [0.38, 0, top + 0.7], [0.1, back * 0.6, top + 0.15], [0, back, top]], 0.05, 1, 9, 4), steel),
    beam([-half, back, top], [half, back, top], 0.08, steel),
    part(new THREE.BoxGeometry(2 * half - 0.06, 0.14, 0.55).rotateX(0.2).translate(0, back + 0.06, top - 0.33), pad),
    box([0, -0.02, seat], [2 * half - 0.06, 0.56, 0.16], pad),
    ...[-1, 1].map((s) => beam([s * half, back, top], [s * half, 0.28, seat - 0.05], 0.06, steel)),
    ...[-1, 1].map((s) => beam([s * half, back, top], [s * half, 0.55, top - 0.12], 0.05, steel)),
    beam([-half, 0.55, top - 0.12], [half, 0.55, top - 0.12], 0.05, steel),
    beam([0, 0.55, top - 0.12], [0, 0.62, seat - 0.4], 0.05, steel),
    beam([-0.8, 0.62, seat - 0.4], [0.8, 0.62, seat - 0.4], 0.06, steel),
  ];
}

/** A T-bar wound up under its spring box, its stem right under the box, as it hangs between riders. */
function tbar(seed: number): THREE.BufferGeometry[] {
  const housing = pick(LIVERY, seed, 1);
  const steel = pick(STEEL, seed, 0);
  return [
    box([0, 0, 0], [0.2, 0.4, 0.26], DARK),
    beam([0.05, 0, -0.05], [0.32, 0, -0.25], 0.06, DARK),
    beam([0.32, 0, -0.2], [0.06, 0, -0.62], 0.06, DARK),
    box([0, 0, -0.9], [0.32, 0.42, 0.6], housing),
    beam([0, 0, -1.2], [0, 0, -2.9], 0.06, steel),
    beam([-0.5, 0, -2.9], [0.5, 0, -2.9], 0.07, steel, true),
  ];
}

/**
 * A carrier hanging from its grip at the origin, the cable along y. Hangers bend out to +x past the
 * pylons' sheaves: turn a carrier half round for the track at -x, which runs the other way.
 */
export function cabin(kind: AerialwayKind, seed = 1): THREE.BufferGeometry {
  const build = { cable_car: tram, gondola, chair_lift: chair, drag_lift: tbar }[kind];
  return finish(build(seed));
}
