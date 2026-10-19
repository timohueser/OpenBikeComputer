import * as THREE from 'three';
import type { Animal } from '../library';
import { hex, noise, ramp, random, rampAt, smooth, v3 } from '../shape';
import { cow } from './brown-cattle';

/** Red pied first, as most Fleckvieh are; then the pale gold of the Swiss Simmental, and dark red. */
const SIMMENTAL_COATS = [ramp(['#6e2e14', '#a85428', '#cc7a44']), ramp(['#94602c', '#c88e4c', '#e6b678']), ramp(['#5a2412', '#8a3c1e', '#b0603a'])];
const WHITE = ramp(['#c4bcae', '#ece6da', '#fbf8f0']);
const NOSE = hex('#d49a8a');

/** White splashes on the body by seed: how many, and their mean radius in metres. */
const SPLASHES: [number, number][] = [[6, 0.2], [10, 0.28], [8, 0.22]];

/**
 * A Simmental or Fleckvieh: a red or gold coat with white splashes, a white head with a pink
 * nose, white legs below the knee and hock, a white belly and tail switch; short horns on two
 * coats of three, and the dark red one has red rings round its eyes.
 */
function simmentalCattle(seed: number): THREE.BufferGeometry {
  const coat = SIMMENTAL_COATS[seed % SIMMENTAL_COATS.length];
  const [count, size] = SPLASHES[seed % SPLASHES.length];
  const shade = (p: THREE.Vector3, n: THREE.Vector3) => 0.5 + 0.3 * n.y + 0.05 * noise(p, 11, 3);
  // Splashes over the back, flanks and shoulders, ragged at the edge; legs and belly are white below.
  const rand = random(seed + 7);
  const splashes = Array.from({ length: count }, () => [v3((rand() - 0.5) * 0.9, 0.85 + 0.55 * rand(), -0.9 + 1.9 * rand()), size * (0.7 + 0.6 * rand())] as const);
  const white = (p: THREE.Vector3, n: THREE.Vector3) => {
    const inside = Math.max(...splashes.map(([c, r]) => r - p.distanceTo(c))) + 0.06 * noise(p, 8, seed);
    const legs = smooth(0.48, 0.38, p.y + 0.05 * noise(p, 13, seed));
    const belly = smooth(-0.35, -0.7, n.y) * smooth(0.95, 0.75, p.y);
    return Math.max(smooth(-0.03, 0.03, inside), legs, belly);
  };
  const hide = (p: THREE.Vector3, n: THREE.Vector3) => rampAt(coat, shade(p, n)).lerp(rampAt(WHITE, shade(p, n)), white(p, n));
  const face = (q: THREE.Vector3, n: THREE.Vector3) => {
    if (q.z > 0.47 && q.y > -0.02) return NOSE;
    const ring = seed % 3 === 2 && Math.hypot(Math.abs(q.x) - 0.1, q.z - 0.13) < 0.065;
    return rampAt(ring ? coat : WHITE, 0.5 + 0.3 * n.y);
  };
  return cow({ hide, face, ears: rampAt(coat, 0.5), tassel: hex('#f2eee4'), horns: seed % 3 === 1 ? 0 : 0.18, udder: 0.14 });
}

export default {
  name: 'Simmental cattle',
  latin: 'Bos taurus (Simmental)',
  build: simmentalCattle,
  height_m: 1.45,
} satisfies Animal;
