import * as THREE from 'three';
import type { Animal } from '../library';
import { hex, noise, ramp, random, rampAt, smooth, v3 } from '../shape';
import { cow } from './brown-cattle';

const WHITE = ramp(['#bdbab2', '#e9e6de', '#faf8f2']);
const BLACK = ramp(['#121111', '#1e1c1b', '#363230']);
const MUZZLE = hex('#5e4c4a');

/** A patch's mean radius in metres by seed: about half black, then a darker and a whiter cow. */
const PATCH = [0.3, 0.38, 0.24];

/**
 * A Holstein-Friesian: polled, black patches on white with white legs, belly and tail tassel, a
 * black head with a white blaze, and a full udder.
 */
function dairyCattle(seed: number): THREE.BufferGeometry {
  const size = PATCH[seed % PATCH.length];
  const shade = (p: THREE.Vector3, n: THREE.Vector3) => 0.5 + 0.3 * n.y + 0.05 * noise(p, 11, 3);
  // Round patches with ragged edges, scattered over the model frame so that they run on across
  // body, neck and thighs; they keep off the lower legs and the belly.
  const rand = random(seed + 5);
  const patches = Array.from({ length: 9 }, () => [v3((rand() - 0.5) * 0.9, 0.75 + 0.7 * rand(), -0.95 + 2.1 * rand()), size * (0.75 + 0.5 * rand())] as const);
  const black = (p: THREE.Vector3, n: THREE.Vector3) => {
    const inside = Math.max(...patches.map(([c, r]) => r - p.distanceTo(c))) + 0.07 * noise(p, 7, seed);
    return smooth(-0.04, 0.04, inside) * smooth(0.45, 0.6, p.y) * smooth(-0.75, -0.4, n.y);
  };
  const hide = (p: THREE.Vector3, n: THREE.Vector3) => rampAt(WHITE, shade(p, n)).lerp(rampAt(BLACK, shade(p, n)), black(p, n));
  const face = (q: THREE.Vector3, n: THREE.Vector3) => {
    if (q.z > 0.46 && q.y > -0.03) return MUZZLE;
    const blaze = q.y > 0 && Math.abs(q.x) < 0.03 + 0.06 * smooth(0.1, 0.45, q.z) && q.z > 0.04;
    return rampAt(blaze ? WHITE : BLACK, 0.5 + 0.3 * n.y);
  };
  return cow({ hide, face, ears: rampAt(BLACK, 0.5), tassel: hex('#f0eee8'), horns: 0, udder: 0.17 });
}

export default {
  name: 'Holstein-Friesian cattle',
  latin: 'Bos taurus (Holstein-Friesian)',
  build: dairyCattle,
  height_m: 1.45,
} satisfies Animal;
