import * as THREE from 'three';
import type { Animal } from '../library';
import { ball, hex, limb, merge, noise, paint, place, ramp, rampAt, smooth, soft, tube, type Vec } from '../shape';

/** Pied Majorera, near-black Tinerfeña, red Palmera: base coat, then patch colour and share. */
const GOAT_COATS: [THREE.Color[], THREE.Color, number][] = [
  [ramp(['#1e1814', '#2e241e', '#4a3a30']), hex('#ece4d4'), 0.45],
  [ramp(['#1a1512', '#2a211c', '#3e3128']), hex('#5a4434'), 0.25],
  [ramp(['#5a2e18', '#87482a', '#b06c40']), hex('#e6dccb'), 0.2],
];
// Paler at the base than the darkest coat, so the horns part from the head.
const GOAT_HORN = hex('#6b5c4a');
const GOAT_HORN_TIP = hex('#a89a80');

const INK = hex('#2e211b');
const HOOF = hex('#2c2420');

/** A Canarian goat: lean and long in the leg, short swept-back horns and a wisp of beard. */
function goat(seed: number): THREE.BufferGeometry {
  const [base, patch, share] = GOAT_COATS[seed % GOAT_COATS.length];
  // Patches follow the model frame, so they run on across body, neck and legs.
  const hide = (p: THREE.Vector3, n: THREE.Vector3) =>
    rampAt(base, 0.5 + 0.3 * n.y + 0.15 * noise(p, 14, 1)).lerp(patch, smooth(1 - share, 1.03 - share, 0.5 + 0.4 * noise(p, 6, 4 + seed)));
  const body = (geo: THREE.BufferGeometry, at: Vec, scale: Vec, rot: Vec = [0, 0, 0]) => paint(soft(place(geo, { at, scale, rot }), 0.01, 9, 2), hide);
  // Rings every few centimetres keep a patch's edge crisp down a leg.
  const leg = (points: Vec[], r: number, tip: number) => paint(soft(tube(points, r, tip, 6, 7)), hide);
  const [hy, hz] = [0.86, 0.5];
  const parts = [
    body(ball(1, 16, 10), [0, 0.6, -0.04], [0.18, 0.17, 0.38]),
    // A deeper chest than rump, as in a lean dairy goat, and a hip for the thighs to hang from.
    body(ball(1, 12, 8), [0, 0.58, 0.18], [0.175, 0.2, 0.2]),
    body(ball(1, 12, 8), [0, 0.61, -0.25], [0.185, 0.16, 0.17]),
    // The neck tapers to the head and sinks deep into the chest, so no seam rings it.
    leg([[0, 0.56, 0.18], [0, 0.73, 0.33], [0, 0.86, 0.49]], 0.1, 0.6),
    body(ball(1, 10, 7), [0, hy, hz], [0.07, 0.075, 0.16], [0.8, 0, 0]),
    paint(soft(place(new THREE.ConeGeometry(0.024, 0.1, 5, 1, true), { at: [0, hy - 0.14, hz + 0.06], rot: [Math.PI - 0.3, 0, 0] })), base[0]),
    limb([0, 0.68, -0.38], [0, 0.84, -0.45], 0.024, hide, 0, 5),
  ];
  for (const s of [-1, 1]) {
    const horn: Vec[] = [[0.025, 0.07, -0.06], [0.035, 0.14, -0.09], [0.055, 0.19, -0.15], [0.075, 0.2, -0.23]].map(([x, y, z]) => [x * s, hy + y, hz + z] as Vec);
    parts.push(
      paint(soft(tube(horn, 0.024, 0.3, 6, 5)), (p) => GOAT_HORN.clone().lerp(GOAT_HORN_TIP, smooth(hz - 0.12, hz - 0.22, p.z))),
      body(ball(1, 6, 4), [0.09 * s, hy + 0.02, hz - 0.07], [0.09, 0.02, 0.04], [0, 0.6 * s, -0.7 * s]),
      place(paint(soft(ball(0.013, 6, 4)), INK), { at: [0.056 * s, hy + 0.03, hz + 0.02] }),
      leg([[0.09 * s, 0.52, 0.22], [0.09 * s, 0.022, 0.25]], 0.052, 0.68),
      // The hind leg comes down from a thick thigh and bends back at the hock, whose round top
      // closes the thigh's open end.
      leg([[0.095 * s, 0.62, -0.23], [0.095 * s, 0.42, -0.29], [0.09 * s, 0.25, -0.36]], 0.08, 0.42),
      limb([0.09 * s, 0.25, -0.36], [0.09 * s, 0.022, -0.32], 0.035, hide, 0.003, 8),
    );
    for (const z of [0.258, -0.312]) parts.push(place(paint(soft(ball(1, 8, 5)), HOOF), { at: [0.09 * s, 0.022, z], scale: [0.045, 0.026, 0.052] }));
  }
  return merge(parts).rotateX(Math.PI / 2);
}

export default {
  name: 'Canarian goat',
  latin: 'Capra hircus',
  build: goat,
  height_m: 1.05,
} satisfies Animal;
