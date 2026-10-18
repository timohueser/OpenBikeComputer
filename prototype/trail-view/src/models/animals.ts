/**
 * Animals, 1 tall with their feet at the origin and z up. Each is built y up facing +z, as in the
 * marmot session's generator (`models.js`) the sheep comes from, then turned.
 */

import * as THREE from 'three';
import { ball, hash, hex, lathe, limb, merge, noise, paint, place, random, smooth, soft, tube, unit, v3, type Vec } from './shape';

const FLEECE = hex('#efe7d6');
const FLEECE_SHADE = hex('#c9bb9f');
const BLACKFACE = hex('#2d2828');
const MUZZLE_GREY = hex('#5d5550');
const HORN_IVORY = hex('#cdb994');
const HORN_TIP = hex('#8f7a5a');
const AMBER = hex('#d9a441');
const INK = hex('#2e211b');

/** A Scottish Blackface sheep standing on all fours, as on Irish and Scottish hills. */
function sheep(): THREE.BufferGeometry {
  const [hx, hy, hz] = [0, 1.24, 0.84];
  const head = (x: number, y: number, z: number): Vec => [hx + x, hy + y, hz + z];
  const wool = (p: THREE.Vector3, n: THREE.Vector3) => FLEECE_SHADE.clone().lerp(FLEECE, smooth(-0.5, 0.6, noise(p, 13, 3) + 0.4 * n.y));
  const face = (p: THREE.Vector3) => BLACKFACE.clone().lerp(MUZZLE_GREY, smooth(hz + 0.11, hz + 0.2, p.z));
  const horn = (p: THREE.Vector3) => HORN_IVORY.clone().lerp(HORN_TIP, smooth(0.15, 0.26, Math.abs(p.x)));
  const fluff = (r: number, at: Vec, scale: Vec = [1, 1, 1], seed = 5) => paint(place(soft(ball(r, 16, 12), 0.03, 13, seed), { at, scale }), wool);
  const parts = [
    paint(soft(ball(1, 28, 20).scale(0.5, 0.42, 0.7).translate(0, 0.98, -0.02), 0.05, 13, 3), wool),
    fluff(0.27, [0, 1.2, 0.52]),
    fluff(0.1, [0, 0.98, -0.74], [1, 1.3, 0.8], 6),
    fluff(0.11, head(0, 0.15, -0.12), [1, 1, 1], 7),
    paint(place(soft(ball(1, 16, 12).scale(0.13, 0.15, 0.24)), { at: head(0, 0, 0), rot: [0.55, 0, 0] }), face),
  ];
  for (const s of [-1, 1]) {
    // The horn curls from the crown back, down and forward around the ear.
    const curl: Vec[] = [];
    for (let i = 0; i <= 10; i++) {
      const t = i / 10;
      const a = Math.PI / 2 + t * 1.9 * Math.PI;
      const r = 0.13 - 0.08 * t;
      curl.push(head(s * (0.08 + 0.17 * t), r * Math.sin(a), -0.12 + r * Math.cos(a)));
    }
    parts.push(
      paint(soft(tube(curl, 0.045, 0.35, 20, 6)), horn),
      place(paint(soft(ball(1, 10, 8).scale(0.09, 0.025, 0.045)), BLACKFACE), { at: head(0.19 * s, -0.02, -0.12), rot: [0, 0, -0.25 * s] }),
      place(paint(soft(ball(0.028, 8, 6)), AMBER), { at: head(0.108 * s, 0.05, 0.04) }),
    );
    for (const z of [0.36, -0.38]) {
      parts.push(
        limb([0.2 * s, 0.7, z], [0.2 * s, 0.06, z], 0.05, BLACKFACE, 0.01, 8),
        place(paint(soft(ball(0.06, 8, 6)), INK), { at: [0.2 * s, 0.03, z + 0.02], scale: [1, 0.6, 1.25] }),
      );
    }
  }
  // y up facing +z, to z up facing -y.
  return merge(parts).rotateX(Math.PI / 2);
}

/** Shade, body and sun-bleached tip of a coat. */
type Coat = [THREE.Color, THREE.Color, THREE.Color];

const hues = (...h: [string, string, string]) => h.map(hex) as Coat;

/** Ginger first, as most Highland cattle are; then dark brown and dun. */
const HIGHLAND_COATS: Coat[] = [hues('#7e3214', '#b3561f', '#e09048'), hues('#2a1810', '#4a2a1a', '#7a4a2c'), hues('#9a7448', '#d0ae74', '#efd6a2')];
const HIGHLAND_HORN = hex('#e9dcbc');
const HIGHLAND_HORN_TIP = hex('#4a3a2c');
const NOSE = hex('#4a3430');
const HOOF = hex('#2c2420');

/** The three-stop ramp of a coat at `t`, clamped to [0, 1]. */
function ramp([shade, body, tip]: Coat, t: number): THREE.Color {
  const u = Math.min(1, Math.max(0, t));
  return u < 0.5 ? shade.clone().lerp(body, u * 2) : body.clone().lerp(tip, u * 2 - 1);
}

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
    return ramp(fur, 0.4 + lift + 0.15 * p.y + 0.8 * (hash(v3(k), seed) - 0.5));
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
    place(paint(soft(ball(1, 12, 8)), (_, n) => (n.z > 0.3 ? NOSE : ramp(fur, 0.5))), { at: head(0, 0.2, -0.01), scale: [0.16, 0.1, 0.08], rot: [tilt, 0, 0] }),
    coat(ball(1, 8, 6), 8, head(0, -0.17, 0.11), [0.17, 0.08, 0.12], [tilt, 0, 0], 0.4),
    // A belly closes the coat for an eye downhill.
    paint(soft(place(new THREE.CircleGeometry(1, 12), { at: [0, 0.5, -0.05], rot: [Math.PI / 2, 0, 0], scale: [0.4, 0.72, 1] })), ramp(fur, 0.1)),
    // The tail hangs against the rump to a dark tuft at the hock.
    paint(soft(tube([[0, 0.98, -0.68], [0, 0.9, -0.8], [0, 0.72, -0.86], [0, 0.5, -0.87]], 0.03, 0.7, 6, 5)), ramp(fur, 0.25)),
    paint(soft(place(lathe([[0, 0], [0.04, 0.06], [0.05, 0.13], [0.03, 0.22], [0, 0.26]], 6, 8), { at: [0, 0.27, -0.875], rot: [-Math.PI / 2, 0, 0] }), 0.008, 30, 2), ramp(fur, 0.08)),
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
    parts.push(paint(soft(tube(lock, 0.038 + 0.016 * rand(), 0.3, 5, 5)), ramp(fur, 0.7 + 0.3 * rand())));
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
        paint(soft(tube([[0.22 * s, 0.62, z], [0.22 * s, 0.04, z]], 0.095, 0.72, 2, 7)), ramp(fur, 0.55)),
        place(paint(soft(ball(1, 8, 5)), HOOF), { at: [0.22 * s, 0.035, z + 0.01], scale: [0.085, 0.045, 0.095] }),
      );
    }
  }
  return merge(parts).rotateX(Math.PI / 2);
}

/** Pied Majorera, near-black Tinerfeña, red Palmera: base coat, then patch colour and share. */
const GOAT_COATS: [Coat, THREE.Color, number][] = [
  [hues('#1e1814', '#2e241e', '#4a3a30'), hex('#ece4d4'), 0.45],
  [hues('#1a1512', '#2a211c', '#3e3128'), hex('#5a4434'), 0.25],
  [hues('#5a2e18', '#87482a', '#b06c40'), hex('#e6dccb'), 0.2],
];
// Paler at the base than the darkest coat, so the horns part from the head.
const GOAT_HORN = hex('#6b5c4a');
const GOAT_HORN_TIP = hex('#a89a80');

/** A Canarian goat: lean and long in the leg, short swept-back horns and a wisp of beard. */
function goat(seed: number): THREE.BufferGeometry {
  const [base, patch, share] = GOAT_COATS[seed % GOAT_COATS.length];
  // Patches follow the model frame, so they run on across body, neck and legs.
  const hide = (p: THREE.Vector3, n: THREE.Vector3) =>
    ramp(base, 0.5 + 0.3 * n.y + 0.15 * noise(p, 14, 1)).lerp(patch, smooth(1 - share, 1.03 - share, 0.5 + 0.4 * noise(p, 6, 4 + seed)));
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

const ANIMALS = { sheep, 'highland-cow': highlandCow, goat };

export type AnimalName = keyof typeof ANIMALS;

/** `seed` picks the coat where a breed has several. */
export const animal = (name: AnimalName, seed = 0) => unit(ANIMALS[name](seed));
