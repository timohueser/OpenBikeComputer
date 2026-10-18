import * as THREE from 'three';
import type { Animal } from '../library';
import { ball, hex, merge, noise, paint, place, ramp, rampAt, smooth, soft, tube, type Vec } from '../shape';
import { loft } from './brown-cattle';

/** Bay with black points, chestnut, grey and black: coat, then legs below the knee and the mane and tail. */
const HORSE_COATS: [THREE.Color[], THREE.Color, THREE.Color][] = [
  [ramp(['#3a1c0e', '#6a3418', '#965630']), hex('#1a1512'), hex('#1a1512')],
  [ramp(['#56240e', '#8a4018', '#b46632']), hex('#6e3014'), hex('#6a3014')],
  [ramp(['#7e7e7c', '#b8b6b2', '#e2e0dc']), hex('#6e6c6a'), hex('#9a9894')],
  [ramp(['#141212', '#221f1e', '#3a3534']), hex('#141212'), hex('#100e0e')],
];
const HOOF = hex('#2a2624');
const EYE = hex('#141010');

/**
 * A horse in metres, 1.55 m at the withers: a barrel on long thin legs, a neck raised at about 50
 * degrees under a mane, a long head held toward the vertical, upright ears and a full tail.
 */
function horse(seed: number): THREE.BufferGeometry {
  const [coat, points, hair] = HORSE_COATS[seed % HORSE_COATS.length];
  // Darker toward the points below knee and hock.
  const hide = (p: THREE.Vector3, n: THREE.Vector3) => rampAt(coat, 0.5 + 0.3 * n.y + 0.06 * noise(p, 9, 1)).lerp(points, smooth(0.62, 0.4, p.y));
  // The neck rises from the shoulders to the poll; its `up` side is the crest.
  const [ny, nz, rise] = [1.3, 0.44, 0.86];
  const neck = (x: number, t: number, s: number): Vec => [x, ny + t * Math.sin(rise) + s * Math.cos(rise), nz + t * Math.cos(rise) - s * Math.sin(rise)];
  // The head hangs from the poll toward the muzzle; `s` is out of the face.
  const [py, pz, tilt] = [ny + 0.8 * Math.sin(rise), nz + 0.8 * Math.cos(rise) + 0.04, 1.05];
  const head = (x: number, t: number, s: number): Vec => [x, py - t * Math.sin(tilt) + s * Math.cos(tilt), pz + t * Math.cos(tilt) + s * Math.sin(tilt)];
  const parts = [
    // Buttocks, croup, loin, barrel, withers, shoulder and chest.
    paint(
      soft(
        loft(
          [
            [-0.86, 1.3, 0, 0, 0, 2],
            [-0.8, 1.3, 0.17, 0.2, 0.3, 2.2],
            [-0.62, 1.26, 0.25, 0.28, 0.38, 2.4],
            [-0.32, 1.24, 0.27, 0.26, 0.36, 2.4],
            [0.0, 1.23, 0.28, 0.26, 0.38, 2.4],
            [0.3, 1.24, 0.26, 0.31, 0.39, 2.3],
            [0.55, 1.24, 0.21, 0.28, 0.36, 2.2],
            [0.72, 1.26, 0.15, 0.2, 0.28, 2],
            [0.8, 1.3, 0, 0, 0, 2],
          ],
          26,
          16,
        ),
      ),
      hide,
    ),
    // The neck, deep at the shoulders and narrow at the throat.
    paint(
      soft(
        place(
          loft(
            [
              [-0.06, 0, 0, 0, 0, 2],
              [0, 0, 0.17, 0.2, 0.26, 2.2],
              [0.25, 0, 0.14, 0.14, 0.18, 2.2],
              [0.55, 0, 0.12, 0.11, 0.15, 2.2],
              [0.8, 0, 0.11, 0.1, 0.15, 2],
              [0.88, 0, 0, 0, 0, 2],
            ],
            12,
            12,
          ),
          { at: neck(0, 0, 0), rot: [-rise, 0, 0] },
        ),
      ),
      hide,
    ),
    // A broad forehead over deep jowls, a long straight face and a soft muzzle.
    paint(
      soft(
        place(
          loft(
            [
              [-0.05, 0, 0, 0, 0, 2],
              [0.02, 0, 0.11, 0.07, 0.16, 2.2],
              [0.12, 0, 0.11, 0.075, 0.16, 2.2],
              [0.27, 0, 0.085, 0.065, 0.1, 2.2],
              [0.46, 0, 0.07, 0.06, 0.07, 2.2],
              [0.56, 0, 0.075, 0.065, 0.075, 2.2],
              [0.61, 0, 0, 0, 0, 2],
            ],
            12,
            12,
          ),
          { at: head(0, 0, 0), rot: [tilt, 0, 0] },
        ),
      ),
      // The muzzle darkens toward the points.
      (p, n) => hide(p, n).lerp(points, 0.6 * smooth(py - 0.4, py - 0.5, p.y)),
    ),
    // The mane falls from the crest to one side, from the poll to the withers.
    paint(soft(tube([neck(0.03, 0.86, 0.06), neck(0.05, 0.5, 0.11), neck(0.06, 0.15, 0.17), neck(0.04, -0.12, 0.21)], 0.07, 0.8, 8, 6).scale(0.55, 1, 1)), hair),
    // A full tail from the croup, hanging to the hocks.
    paint(soft(tube([[0, 1.49, -0.78], [0, 1.43, -0.9], [0, 1.15, -0.97], [0, 0.82, -0.96], [0, 0.55, -0.93]], 0.05, 1.6, 8, 6)), hair),
  ];
  for (const s of [-1, 1]) {
    parts.push(
      place(paint(soft(new THREE.ConeGeometry(0.035, 0.14, 5, 1, true)), hide), { at: [0.06 * s, py + 0.08, pz - 0.02], rot: [0.25, 0, -0.2 * s] }),
      place(paint(soft(ball(0.02, 6, 4)), EYE), { at: head(0.095 * s, 0.13, 0.02) }),
      // The front leg falls straight from the elbow; the hind one bends back at the hock.
      paint(soft(tube([[0.15 * s, 0.98, 0.5], [0.15 * s, 0.52, 0.52], [0.15 * s, 0.16, 0.52], [0.15 * s, 0.05, 0.56]], 0.075, 0.5, 8, 7)), hide),
      paint(soft(tube([[0.16 * s, 1.05, -0.48], [0.16 * s, 0.56, -0.72], [0.15 * s, 0.16, -0.66], [0.15 * s, 0.05, -0.62]], 0.11, 0.35, 8, 7)), hide),
    );
    for (const z of [0.57, -0.61]) parts.push(place(paint(soft(ball(1, 8, 5)), HOOF), { at: [0.15 * s, 0.04, z], scale: [0.05, 0.045, 0.06] }));
  }
  return merge(parts).rotateX(Math.PI / 2);
}

export default {
  name: 'Horse',
  latin: 'Equus caballus',
  build: horse,
  height_m: 2.0,
} satisfies Animal;
