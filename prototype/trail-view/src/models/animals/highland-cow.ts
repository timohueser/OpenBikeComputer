import * as THREE from 'three';
import type { Animal } from '../library';
import { ball, hash, hex, lathe, merge, paint, place, ramp, rampAt, random, smooth, soft, tube, v3, type Vec } from '../shape';

/** Ginger first, as most Highland cattle are; then dark brown and dun. */
const HIGHLAND_COATS = [ramp(['#7e3214', '#b3561f', '#e09048']), ramp(['#2a1810', '#4a2a1a', '#7a4a2c']), ramp(['#9a7448', '#d0ae74', '#efd6a2'])];
const HIGHLAND_HORN = hex('#e9dcbc');
const HIGHLAND_HORN_TIP = hex('#4a3a2c');
const NOSE = hex('#4a3430');
const HOOF = hex('#2c2420');

/**
 * A long coat: a dome on a skirt whose hem hangs `reach(a)` below the dome's rim in ragged locks,
 * `a` the angle from the front. The dome's poles face front and back, so its seams meet at the
 * chest and rump and do not spoke out from the top of the back.
 */
function mop(cols: number, rows: number, reach: (a: number) => number, seed: number): THREE.BufferGeometry {
  // Half a sphere turned so its rim is the skirt's top ring of `cols` vertices, which welds.
  const dome = new THREE.SphereGeometry(1, rows, cols / 2, 0, Math.PI).rotateX(-Math.PI / 2);
  const skirt = new THREE.CylinderGeometry(1, 1.04, 1, cols, 1, true).translate(0, -0.5, 0);
  const pos = skirt.getAttribute('position');
  for (let j = 0; j <= cols; j++) pos.setY(cols + 1 + j, -reach((2 * Math.PI * j) / cols) * (0.8 + 0.4 * hash(v3(j % cols), seed)));
  return merge([dome, skirt]);
}

/** A Highland cow: a shaggy coat to the knees, a fringe over the eyes and wide upturned horns. */
function highlandCow(seed: number): THREE.BufferGeometry {
  const fur = HIGHLAND_COATS[seed % HIGHLAND_COATS.length];
  // In a part's own frame each of its `cols` strands falls from a parting along the top and on
  // down the skirt, bleached toward the top. Colour hangs on position alone, so a dome and its
  // skirt weld.
  const hair = (cols: number, lift: number) => (p: THREE.Vector3) => {
    const a = Math.atan2((Math.sign(p.x) || 1) * Math.hypot(p.x, Math.max(0, p.y)), p.z);
    const k = (Math.round((a * cols) / (2 * Math.PI)) + cols) % cols;
    return rampAt(fur, 0.4 + lift + 0.15 * p.y + 0.8 * (hash(v3(k), seed) - 0.5));
  };
  const coat = (geo: THREE.BufferGeometry, cols: number, at: Vec, scale: Vec, rot: Vec = [0, 0, 0], lift = 0, wave = 0.012) =>
    soft(place(paint(geo, hair(cols, lift)), { at, scale, rot }), wave, 9, 3);
  // The head in its own frame: across, along the face toward the nose, and out of the face.
  const [hy, hz, tilt] = [0.82, 0.96, 0.9];
  const head = (x: number, t: number, s: number): Vec => [x, hy - t * Math.sin(tilt) + s * Math.cos(tilt), hz + t * Math.cos(tilt) + s * Math.sin(tilt)];
  const rand = random(seed + 3);
  const parts = [
    // Short at chest and rump, long along the flank, so the legs stand at the coat's corners.
    coat(mop(36, 8, (a) => 1.05 + 0.4 * Math.sin(a) ** 2, 1), 36, [0, 0.8, -0.05], [0.42, 0.36, 0.74], [0, 0, 0], 0, 0.025),
    coat(ball(1, 10, 7), 10, [0, 0.76, 0.56], [0.33, 0.31, 0.3]),
    // Hair over the shoulders lifts the back line a little and joins the head to the body.
    coat(ball(1, 10, 7), 10, [0, 0.98, 0.45], [0.3, 0.2, 0.32], [0.2, 0, 0], 0.05),
    coat(ball(1, 10, 7), 10, [0, hy, hz], [0.19, 0.19, 0.27], [tilt, 0, 0]),
    // A broad muzzle sunk into the head, dark all across its front.
    place(paint(soft(ball(1, 12, 8)), (_, n) => (n.z > 0.3 ? NOSE : rampAt(fur, 0.5))), { at: head(0, 0.2, -0.01), scale: [0.16, 0.1, 0.08], rot: [tilt, 0, 0] }),
    coat(ball(1, 8, 6), 8, head(0, -0.17, 0.11), [0.17, 0.08, 0.12], [tilt, 0, 0], 0.4),
    // A belly closes the coat for an eye downhill.
    paint(soft(place(new THREE.CircleGeometry(1, 12), { at: [0, 0.5, -0.05], rot: [Math.PI / 2, 0, 0], scale: [0.4, 0.72, 1] })), rampAt(fur, 0.1)),
    // The tail hangs against the rump to a dark tuft at the hock.
    paint(soft(tube([[0, 0.98, -0.68], [0, 0.9, -0.8], [0, 0.72, -0.86], [0, 0.5, -0.87]], 0.03, 0.7, 6, 5)), rampAt(fur, 0.25)),
    paint(soft(place(lathe([[0, 0], [0.04, 0.06], [0.05, 0.13], [0.03, 0.22], [0, 0.26]], 6, 8), { at: [0, 0.27, -0.875], rot: [-Math.PI / 2, 0, 0] }), 0.008, 30, 2), rampAt(fur, 0.08)),
  ];
  // The fringe: locks from the poll forward over the brow, hanging past the eyes to the muzzle.
  for (let i = 0; i < 13; i++) {
    const across = (i / 12 - 0.5) * 2.2 + 0.12 * (rand() - 0.5);
    const end = 0.04 + 0.13 * (1 - 0.4 * Math.abs(across)) * (0.6 + 0.4 * rand());
    const lock: Vec[] = [];
    for (let k = 0; k <= 3; k++) {
      const t = -0.19 + ((end + 0.19) * k) / 3;
      // Out from the face, more so at the ends, so the locks lie in a shaggy layer.
      const r = 0.19 * Math.sqrt(1 - (t / 0.27) ** 2) + 0.02 + 0.03 * (k / 3) ** 2 * rand();
      const f = across * (1 + 0.8 * (t + 0.19));
      lock.push(head(r * Math.sin(f), t, r * Math.cos(f)));
    }
    parts.push(paint(soft(tube(lock, 0.038 + 0.016 * rand(), 0.3, 5, 5)), rampAt(fur, 0.7 + 0.3 * rand())));
  }
  for (const s of [-1, 1]) {
    // Out from the poll, then up and a little back, pale to a dark tip.
    const horn: Vec[] = [[0.1, 0, 0], [0.3, 0.02, 0.04], [0.5, 0.08, 0.06], [0.62, 0.2, 0.02], [0.64, 0.33, -0.04]].map(
      ([x, y, z]) => [x * s, hy + 0.12 + y, hz - 0.06 + z] as Vec,
    );
    parts.push(
      paint(soft(tube(horn, 0.06, 0.2, 8, 5)), (p) => HIGHLAND_HORN.clone().lerp(HIGHLAND_HORN_TIP, smooth(hy + 0.37, hy + 0.47, p.y))),
      coat(ball(1, 6, 4), 6, [0.26 * s, hy + 0.03, hz - 0.1], [0.13, 0.04, 0.07], [0, 0, -0.35 * s], 0.1),
    );
    for (const z of [0.45, -0.52]) {
      // A leg flares into the coat and stands on a flat hoof, so only the hoof is dark.
      parts.push(
        paint(soft(tube([[0.22 * s, 0.62, z], [0.22 * s, 0.04, z]], 0.095, 0.72, 2, 7)), rampAt(fur, 0.55)),
        place(paint(soft(ball(1, 8, 5)), HOOF), { at: [0.22 * s, 0.035, z + 0.01], scale: [0.085, 0.045, 0.095] }),
      );
    }
  }
  return merge(parts).rotateX(Math.PI / 2);
}

export default {
  name: 'Highland cow',
  latin: 'Bos taurus (Highland)',
  build: highlandCow,
  height_m: 1.3,
} satisfies Animal;
