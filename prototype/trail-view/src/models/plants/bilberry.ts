import * as THREE from 'three';
import type { Plant } from '../library';
import { domed, GOLDEN, hex, merge, paint, ramp, random, ribbon, sheet, smooth, soft, UP, v3, type Ramp, type Vec } from '../shape';
import type { Season } from '../trees';

/** Bilberry leaves: bright green, flame red to crimson in autumn, shed in winter. */
const BILBERRY: Record<Season, Ramp | 'bare'> = {
  winter: 'bare',
  spring: ['#3e7a2a', '#6eaa3a', '#acd65c'],
  summer: ['#356a28', '#5a9634', '#90c04c'],
  autumn: ['#6e1020', '#b0222a', '#e8582a'],
};

/** Bilberry stems at the foot and the tip: green all year, browner in winter. */
const BILBERRY_STEM: Record<Season, [string, string]> = {
  winter: ['#5a4a32', '#6e7440'],
  spring: ['#4a5a30', '#6a9a3a'],
  summer: ['#4a5a30', '#5e8a36'],
  autumn: ['#5a4a30', '#6e7a38'],
};

/**
 * Bilberry: a broom of thin stems, the inner ones upright and the outer ones leaning, each forking
 * once at an angle and set along most of its length with small pointed leaves held out flat; lit
 * as one round tuft. Winter leaves the green-brown stems bare; summer hangs dark blue berries
 * under the leaves.
 */
function bilberry(season: Season, rand: () => number): THREE.BufferGeometry {
  const leaves = BILBERRY[season];
  const [foot, top] = BILBERRY_STEM[season].map(hex);
  const stem = (p: THREE.Vector3) => foot.clone().lerp(top, smooth(0, 0.7, p.z));
  const [berry, bloom] = [hex('#262c54'), hex('#58628e')];
  const light = domed(v3(0, 0, 0.25), v3(0.5, 0.5, 0.75), 0.4);
  const cards = sheet();
  const parts: THREE.BufferGeometry[] = [];
  const leaf = (at: THREE.Vector3, out: THREE.Vector3) => {
    // Drawn even when bare, so the stems stand the same in every season.
    const [tone, length] = [rand(), 0.17 + 0.05 * rand()];
    if (leaves === 'bare') return;
    const [deep, mid, tip] = ramp(leaves);
    const dir = out.clone().setZ(0).normalize().multiplyScalar(0.94).setZ(0.34);
    const side = v3().crossVectors(dir, UP).setLength(0.055);
    const body = mid.clone().lerp(tip, 0.8 * tone).lerp(deep, 0.5 * (1 - smooth(0.1, 0.6, at.z)));
    const base = body.clone().lerp(deep, 0.5);
    cards.add([at.clone().add(side), at.clone().sub(side), at.clone().addScaledVector(dir, length)], [base, base, body.lerp(tip, 0.3)], light);
  };
  const count = 10;
  let berries = season === 'summer' ? 6 : 0;
  for (let k = 0; k < count; k++) {
    const a = k * GOLDEN + rand() * 0.5;
    const out = v3(Math.cos(a), Math.sin(a), 0);
    const across = v3(-out.y, out.x, 0);
    const tilt = 0.12 + 0.65 * Math.sqrt((k + 0.5) / count);
    const length = (1 - (0.35 * k) / count) * (0.88 + 0.12 * rand());
    const dir = out.clone().multiplyScalar(Math.sin(tilt)).setZ(Math.cos(tilt));
    const root = out.clone().multiplyScalar(0.04);
    const end = root.clone().addScaledVector(dir, length);
    const turn = rand() * Math.PI;
    const flat: Vec = [Math.cos(turn), Math.sin(turn), 0];
    parts.push(paint(ribbon([root.toArray(), end.toArray()], flat, (t) => 0.02 - 0.008 * t, 1), stem));
    // The twig forks off at an angle, so the tuft reads as a broom of angular stems.
    const s = k % 2 ? 1 : -1;
    const kink = root.clone().addScaledVector(dir, 0.4 * length);
    const fork = dir.clone().addScaledVector(across, 0.8 * s).normalize();
    const twig = kink.clone().addScaledVector(fork, 0.45 * length);
    parts.push(paint(ribbon([kink.toArray(), twig.toArray()], flat, () => 0.014, 1), stem));
    for (const [i, t] of [0.3, 0.45, 0.6, 0.75, 0.9, 1].entries()) {
      const at = root.clone().addScaledVector(dir, t * length);
      const spread = across.clone().multiplyScalar(i % 2 ? s : -s).applyAxisAngle(UP, (rand() - 0.5) * 0.8).addScaledVector(out, 0.3);
      leaf(at, spread);
      if (berries && i === 2) {
        berries--;
        const hang = at.clone().addScaledVector(spread.normalize(), 0.04).setZ(at.z - 0.05);
        const ball = soft(new THREE.TetrahedronGeometry(0.04)).translate(hang.x, hang.y, hang.z);
        parts.push(paint(ball, (p) => berry.clone().lerp(bloom, smooth(hang.z - 0.02, hang.z + 0.03, p.z))));
      }
    }
    leaf(kink.clone().lerp(twig, 0.5), across.clone().multiplyScalar(-s).addScaledVector(out, 0.5));
    leaf(twig, fork);
  }
  parts.push(cards.build());
  return merge(parts);
}

export default {
  name: 'Bilberry',
  latin: 'Vaccinium myrtillus',
  build: (season, seed) => bilberry(season, random(seed)),
  ground: { on: ['tree', 'shrub', 'grass'], height: [0.2, 0.45], share: 0.25, own: true, clumped: true },
} satisfies Plant;
