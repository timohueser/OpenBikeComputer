import * as THREE from 'three';
import type { Animal } from '../library';
import { ball, hex, lathe, merge, noise, paint, place, ramp, rampAt, smooth, soft, tube, v3, type Vec } from '../shape';

/** A section across z: centre height, half width, half heights above and below, and squareness (2 is an ellipse). */
type Section = [z: number, y: number, x: number, up: number, down: number, n: number];

/**
 * A body swept along z through sections in x and y, closed where a section's sizes are 0. `rows`
 * sections come from a smooth spline through the given ones, evenly between each pair.
 */
export function loft(sections: Section[], rows: number, sides: number): THREE.BufferGeometry {
  const curve = (pick: (s: Section) => Vec) => new THREE.CatmullRomCurve3(sections.map((s) => v3(...pick(s))), false, 'catmullrom', 0.5);
  const [centre, size] = [curve(([z, y, x]) => [z, y, x]), curve(([, , , up, down, n]) => [up, down, n])];
  const lean = (c: number, n: number) => Math.sign(c) * Math.abs(c) ** (2 / n);
  const position: number[] = [];
  const index: number[] = [];
  for (let i = 0; i <= rows; i++) {
    const [z, y, x] = centre.getPoint(i / rows).toArray();
    const [up, down, n] = size.getPoint(i / rows).toArray();
    for (let j = 0; j < sides; j++) {
      const a = (2 * Math.PI * j) / sides;
      const [s, c] = [Math.sin(a), Math.cos(a)];
      position.push(Math.max(0, x) * lean(s, n), y + Math.max(0, c > 0 ? up : down) * lean(c, n), z);
    }
  }
  for (let i = 0; i < rows; i++) {
    for (let j = 0; j < sides; j++) {
      const [a, b, c, d] = [i * sides + j, i * sides + ((j + 1) % sides), (i + 1) * sides + j, (i + 1) * sides + ((j + 1) % sides)];
      index.push(a, c, b, b, c, d);
    }
  }
  const geo = new THREE.BufferGeometry().setAttribute('position', new THREE.Float32BufferAttribute(position, 3));
  geo.setIndex(index);
  return geo;
}

/** A breed on the shared cattle body. `hide` paints the model frame, `face` the head's own. */
export interface Breed {
  hide: (p: THREE.Vector3, n: THREE.Vector3) => THREE.Color;
  /** In the head's frame: x across, y out of the face, z from the poll to the muzzle. */
  face: (q: THREE.Vector3, n: THREE.Vector3) => THREE.Color;
  ears: THREE.Color;
  /** The tuft at the tail's end. */
  tassel: THREE.Color;
  /** Horn length in metres, 0 where the breed is polled. */
  horns: number;
  /** Half width of the udder in metres. */
  udder: number;
}

const HORN = hex('#e2d6b8');
const HORN_TIP = hex('#3a2e26');
const UDDER = hex('#d8a49a');
const HOOF = hex('#2c2420');
const EYE = hex('#1c1614');

/**
 * A cow in metres, about 1.4 m at the withers: a deep barrel on a straight back, square hips, a
 * short neck with a dewlap, and the head hung low from it. The legs end in dark hooves; the tail
 * hangs to the hock.
 */
export function cow(breed: Breed): THREE.BufferGeometry {
  const body = (geo: THREE.BufferGeometry) => paint(soft(geo), breed.hide);
  // The poll, and the head's axis tilted from forward down to the muzzle.
  const [py, pz, tilt] = [1.2, 1.08, 0.9];
  const head = (x: number, t: number, s: number): Vec => [x, py - t * Math.sin(tilt) + s * Math.cos(tilt), pz + t * Math.cos(tilt) + s * Math.sin(tilt)];
  const parts = [
    // Pin bones, hips, flank, barrel, heart girth, shoulders, then the neck to the poll.
    body(
      loft(
        [
          [-0.94, 1.16, 0, 0, 0, 2],
          [-0.9, 1.14, 0.22, 0.22, 0.38, 2.8],
          [-0.72, 1.1, 0.31, 0.31, 0.42, 3.2],
          [-0.4, 1.02, 0.34, 0.38, 0.42, 3],
          [-0.05, 0.99, 0.36, 0.42, 0.41, 2.8],
          [0.3, 1.0, 0.33, 0.42, 0.38, 2.6],
          [0.58, 1.02, 0.25, 0.4, 0.4, 2.4],
          [0.82, 1.06, 0.17, 0.29, 0.4, 2.2],
          [1.0, 1.12, 0.14, 0.2, 0.25, 2],
          [1.1, 1.17, 0.12, 0.13, 0.15, 2],
          [1.15, 1.19, 0, 0, 0, 2],
        ],
        30,
        18,
      ),
    ),
    // A broad brow over deep cheeks, narrowing to a wide muzzle.
    place(
      paint(
        soft(
          loft(
            [
              [-0.04, 0, 0, 0, 0, 2],
              [0.02, 0, 0.13, 0.07, 0.12, 2.6],
              [0.12, 0, 0.135, 0.08, 0.13, 2.6],
              [0.27, 0, 0.11, 0.07, 0.11, 2.4],
              [0.42, 0, 0.095, 0.065, 0.08, 2.4],
              [0.5, 0, 0.1, 0.07, 0.07, 2.4],
              [0.53, 0, 0, 0, 0, 2],
            ],
            12,
            14,
          ),
        ),
        breed.face,
      ),
      { at: head(0, 0, 0), rot: [tilt, 0, 0] },
    ),
    // The tail hangs from the tail head close down the rump to a tassel at the hock.
    paint(soft(tube([[0, 1.36, -0.88], [0, 1.28, -0.97], [0, 1.0, -1.0], [0, 0.66, -0.98]], 0.035, 0.6, 6, 5)), breed.hide),
    paint(soft(place(lathe([[0, 0], [0.045, 0.06], [0.055, 0.14], [0.03, 0.22], [0, 0.25]], 6, 8), { at: [0, 0.45, -0.98], rot: [-Math.PI / 2, 0, 0] }), 0.008, 30, 2), breed.tassel),
  ];
  if (breed.udder) parts.push(place(paint(soft(ball(1, 10, 7)), UDDER), { at: [0, 0.64, -0.5], scale: [breed.udder, 0.75 * breed.udder, 1.2 * breed.udder] }));
  for (const s of [-1, 1]) {
    parts.push(
      // An ear sticks out flat from under the poll.
      place(paint(soft(ball(1, 8, 5)), breed.ears), { at: head(0.17 * s, 0.07, -0.03), scale: [0.11, 0.03, 0.06], rot: [tilt, 0, -0.25 * s] }),
      place(paint(soft(ball(0.02, 6, 4)), EYE), { at: head(0.1 * s, 0.13, 0.025) }),
      // The front leg falls straight from the elbow; the hind one bends back at the hock.
      paint(soft(tube([[0.17 * s, 0.9, 0.46], [0.17 * s, 0.45, 0.49], [0.17 * s, 0.06, 0.49]], 0.095, 0.5, 6, 7)), breed.hide),
      paint(soft(tube([[0.19 * s, 1.0, -0.56], [0.2 * s, 0.7, -0.68], [0.19 * s, 0.47, -0.77], [0.18 * s, 0.06, -0.69]], 0.13, 0.36, 8, 7)), breed.hide),
    );
    for (const z of [0.5, -0.69]) parts.push(place(paint(soft(ball(1, 8, 5)), HOOF), { at: [0.175 * s, 0.035, z + 0.01], scale: [0.06, 0.04, 0.07] }));
    if (breed.horns) {
      // Out from the poll, then up and forward to a dark tip.
      const l = breed.horns;
      const horn: Vec[] = [[0.06, 0, 0], [0.06 + 0.45 * l, 0.15 * l, -0.05 * l], [0.06 + 0.7 * l, 0.55 * l, 0.1 * l], [0.06 + 0.65 * l, 0.9 * l, 0.3 * l]].map(
        ([x, y, z]) => [x * s, py + 0.04 + y, pz - 0.03 + z] as Vec,
      );
      parts.push(paint(soft(tube(horn, 0.035, 0.3, 6, 5)), (p) => HORN.clone().lerp(HORN_TIP, smooth(py + 0.04 + 0.55 * l, py + 0.04 + 0.9 * l, p.y))));
    }
  }
  return merge(parts).rotateX(Math.PI / 2);
}

/** Brown Swiss grey-brown, chestnut as the Asturian and Limousin, and the fawn of the Rubia Gallega. */
const BROWN_COATS = [ramp(['#4a3a30', '#7e6a58', '#a8957e']), ramp(['#5a2a14', '#8e4a24', '#b87444']), ramp(['#7a5530', '#ae8250', '#d4ae78'])];
const BROWN_NOSE = hex('#2e2624');
const BROWN_TASSEL = hex('#2a201a');

/** A plain brown or fawn cow, paler on the belly and round the muzzle; short horns on two coats of three. */
function brownCattle(seed: number): THREE.BufferGeometry {
  const coat = BROWN_COATS[seed % BROWN_COATS.length];
  const pale = rampAt(coat, 1).lerp(hex('#e8dcc8'), 0.4);
  const hide = (p: THREE.Vector3, n: THREE.Vector3) =>
    rampAt(coat, 0.5 + 0.25 * n.y + 0.08 * noise(p, 9, 2)).lerp(pale, 0.6 * smooth(0.25, -0.6, n.y) * smooth(0.5, 0.75, p.y));
  // A pale ring between the dark nose and the face.
  const face = (q: THREE.Vector3, n: THREE.Vector3) =>
    q.z > 0.47 && q.y > -0.02 ? BROWN_NOSE : rampAt(coat, 0.45 + 0.25 * n.y).lerp(pale, smooth(0.36, 0.44, q.z) * 0.9);
  return cow({ hide, face, ears: rampAt(coat, 0.4), tassel: BROWN_TASSEL, horns: seed % 3 === 2 ? 0 : 0.2, udder: 0.1 });
}

export default {
  name: 'Brown cattle',
  latin: 'Bos taurus (brown)',
  build: brownCattle,
  height_m: 1.4,
} satisfies Animal;
