import * as THREE from 'three';
import type { Plant } from '../library';
import { blade, GOLDEN, hash, hex, merge, paint, ramp, random, smooth, soft, UP, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Silver rosette leaves of the tajinaste. */
const SILVER: Ramp = ['#5e6a5a', '#9aa496', '#cdd2c4'];

/** The spike: coral red in flower (buds, flowers, bract tips), spent by late summer, dry in winter. */
const SPIKE: Record<Season, Ramp> = {
  winter: ['#5a4434', '#7e6048', '#a08262'],
  spring: ['#8e3036', '#d8505a', '#f08a84'],
  summer: ['#6a3a2c', '#9a5a40', '#c08262'],
  autumn: ['#5e4636', '#84644a', '#a68866'],
};

/**
 * A mound of narrow silver leaves round the foot, leaving at `upright` from vertical in the
 * middle to `spread` at the rim and arching out: 3 triangles a leaf.
 */
function rosette(leaves: number, reach: number, [upright, spread]: [number, number], rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(SILVER);
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < leaves; k++) {
    const a = k * GOLDEN + rand() * 0.4;
    const tilt = upright + (spread - upright) * Math.sqrt((k + 0.5) / leaves) * (0.85 + 0.15 * rand());
    const length = reach * (0.8 + 0.3 * rand());
    const geo = blade([0.04 * Math.cos(a), 0.04 * Math.sin(a), 0], a, length, tilt, 0.35 * tilt, (t) => 0.3 * reach * Math.sin(Math.PI * Math.min(1, 0.15 + t)), 2);
    parts.push(paint(geo, (p) => deep.clone().lerp(mid, smooth(0, 0.4 * length, p.length())).lerp(tip, smooth(0.5 * length, length, p.length()))));
  }
  return merge(parts);
}

/**
 * Teide bugloss in flower: one tall narrow spike out of a silver rosette, its foot still leafy
 * and silver. The spike is a stack of twelve small flaring tiers of flowers, each turned half a
 * corner against the one below, so its outline is bristly from every side. 315 triangles.
 */
function tajinaste(season: Season, seed: number, rand: () => number): THREE.BufferGeometry {
  const [, mid] = ramp(SILVER);
  const [dark, body, lit] = ramp(SPIKE[season]);
  const bract = hex('#b4bcac');
  const [tiers, sides] = [12, 5];
  const girth = (z: number) => 0.085 * (1.05 - z) ** 0.8;
  // A tier flares out to its lit lower edge and narrows into shade at the foot of the next.
  const profile = [new THREE.Vector2(0.075, 0), new THREE.Vector2(0.085, 0.1)];
  for (let t = 0; t < tiers; t++) {
    const z = 0.13 + (t / tiers) * 0.82;
    profile.push(new THREE.Vector2((1.2 + 0.2 * rand()) * girth(z), z), new THREE.Vector2(0.85 * girth(z), z + 0.045));
  }
  profile.push(new THREE.Vector2(0, 1));
  const spike = new THREE.LatheGeometry(profile, sides).rotateX(Math.PI / 2);
  const pos = spike.getAttribute('position');
  const colour = new Float32Array(pos.count * 3);
  const p = v3();
  for (let i = 0; i < pos.count; i++) {
    const row = i % profile.length;
    const edge = row >= 2 && row % 2 === 0;
    p.fromBufferAttribute(pos, i).applyAxisAngle(UP, (Math.floor(row / 2) * Math.PI) / sides);
    const c = edge ? body.clone().lerp(lit, 0.2 + 0.6 * smooth(0.4, 1, p.z)) : dark.clone().lerp(body, 0.5);
    if (edge && hash(p, seed + 3) < 0.3) c.lerp(bract, 0.5);
    c.lerp(mid, 1 - smooth(0.1, 0.24, p.z)).toArray(colour, i * 3);
    pos.setXYZ(i, p.x, p.y, p.z);
  }
  spike.setAttribute('color', new THREE.BufferAttribute(colour, 3));
  return merge([rosette(20, 0.28, [0.4, 1.15], rand), soft(spike)]);
}

export default {
  name: 'Tajinaste',
  latin: 'Echium wildpretii',
  build: (season, seed) => tajinaste(season, seed, random(seed)),
  ground: { on: ['shrub', 'bare'], height: [1.5, 2.5], share: 0.02, own: true },
} satisfies Plant;
