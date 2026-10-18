import * as THREE from 'three';
import type { Animal } from '../library';
import { ball, hex, limb, merge, noise, paint, place, smooth, soft, tube, type Vec } from '../shape';

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

export default {
  name: 'Scottish Blackface sheep',
  latin: 'Ovis aries',
  build: sheep,
  height_m: 0.95,
} satisfies Animal;
