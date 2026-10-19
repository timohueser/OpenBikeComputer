import * as THREE from 'three';
import type { Plant } from '../library';
import { blade, domed, GOLDEN, hex, merge, paint, ramp, random, sheet, smooth, UP, v3, type Ramp, type Sheet } from '../shape';
import type { Season } from '../trees';

/** The rosette's keeled leaves: grey-green from autumn to spring, dead straw by summer. */
const LEAVES: Record<Season, Ramp> = {
  winter: ['#4a6444', '#7c9a6c', '#a6bc92'],
  spring: ['#4a6444', '#7c9a6c', '#a6b088'],
  summer: ['#6a5034', '#9c7e52', '#c6aa7a'],
  autumn: ['#4e7044', '#84a86c', '#aac890'],
};

/** Leaf length in metres of a one-metre plant, and how far from upright it leans and droops. */
const ROSETTE: Record<Season, [length: number, tilt: number, arch: number]> = {
  winter: [0.5, 0.75, 0.35],
  spring: [0.55, 0.85, 0.5],
  summer: [0.45, 1.25, 0.5],
  autumn: [0.28, 0.6, 0.25],
};

/**
 * The stalk, the colour of what sits along its racemes and the buds at their tips. Winter has the
 * new stalk in bud, spring white stars, summer the dry stalk with brown capsules, autumn last
 * year's stalk weathered grey.
 */
const STALK: Record<Season, [stalk: string, along: string, buds: string]> = {
  winter: ['#6a8650', '#7e8a5a', '#9a6e60'],
  spring: ['#6a8650', '#fbf8f2', '#b07c6c'],
  summer: ['#8a6c48', '#6a5034', '#7a6040'],
  autumn: ['#8e877a', '#a49884', '#948a7a'],
};

/** A flat six-pointed star of two triangles at `at`, facing `n`: a flower, or a capsule when small. */
function star(cards: Sheet, at: THREE.Vector3, n: THREE.Vector3, r: number, colour: THREE.Color, turn: number): void {
  const u = v3().crossVectors(n, Math.abs(n.z) > 0.9 ? v3(1, 0, 0) : UP).normalize();
  const w = v3().crossVectors(n, u);
  // Lit as if turned halfway to the sky, so a raceme's stars stay bright from every side.
  const lit = n.clone().lerp(UP, 0.5).normalize();
  const tip = (a: number) => at.clone().addScaledVector(u, r * Math.cos(a)).addScaledVector(w, r * Math.sin(a));
  for (const s of [0, Math.PI / 3]) {
    const corners = [0, 1, 2].map((k) => tip(turn + s + (k * 2 * Math.PI) / 3));
    cards.add(corners, [colour, colour, colour], () => lit);
  }
}

/**
 * Branched asphodel: a fountain of long keeled leaves, and out of it one stalk that branches near
 * the top into a candelabra of racemes. Each raceme carries stars along its upper part and a slim
 * cluster of buds at its tip; the stalk keeps its height in every season. 176 triangles.
 */
function asphodel(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(LEAVES[season]);
  const [stalk, along, buds] = STALK[season].map(hex);
  const [length, tilt, arch] = ROSETTE[season];
  const parts: THREE.BufferGeometry[] = [];
  // Spring leaves have dried at the tips by flowering time.
  const dryTip = season === 'spring' ? hex('#a88a5a') : tip;
  for (let k = 0; k < 16; k++) {
    const a = k * GOLDEN + rand() * 0.4;
    const l = length * (0.75 + 0.35 * rand());
    // A strap, widest past its middle, that narrows to a point over its last quarter.
    const width = (t: number) => 0.07 * Math.min(1, 4 * (1 - t)) * (0.8 + 0.2 * Math.sin(Math.PI * t));
    const leaf = blade([0, 0, 0], a, l, tilt * (0.5 + 0.7 * rand()), arch * (0.8 + 0.4 * rand()), width, 3);
    parts.push(paint(leaf, (p) => {
      const t = Math.hypot(p.x, p.y, p.z) / l;
      return deep.clone().lerp(mid, smooth(0, 0.35, t)).lerp(dryTip, smooth(0.6, 1, t));
    }));
  }
  const cards = sheet();
  const light = domed(v3(0, 0, 0.6), v3(0.3, 0.3, 0.5), 0.5);
  // Two crossed slim triangles, so an axis shows from every side.
  const axis = (from: THREE.Vector3, to: THREE.Vector3, width: number, foot: THREE.Color, top: THREE.Color) => {
    const dir = to.clone().sub(from).normalize();
    const across = v3().crossVectors(dir, UP).normalize();
    for (const side of [across, v3().crossVectors(dir, across)]) {
      cards.add([from.clone().addScaledVector(side, width), from.clone().addScaledVector(side, -width), to], [foot, foot, top], light);
    }
  };
  const lean = rand() * 2 * Math.PI;
  const top = v3(0.05 * Math.cos(lean), 0.05 * Math.sin(lean), 1);
  axis(v3(), top, 0.02, stalk.clone().lerp(deep, 0.3), stalk);
  const racemes: [THREE.Vector3, THREE.Vector3, number][] = [[v3().lerp(top, 0.6), top, 9]];
  for (let b = 0; b < 4; b++) {
    const at = v3().lerp(top, 0.46 + 0.06 * b + 0.04 * rand());
    const a = lean + Math.PI / 2 + b * 1.7 + rand() * 0.5;
    const reach = 0.17 + 0.09 * rand();
    const end = at.clone().add(v3(reach * Math.cos(a), reach * Math.sin(a), 0.3 + 0.12 * rand()));
    axis(at, end, 0.012, stalk, stalk);
    racemes.push([at.clone().lerp(end, 0.25), end, 6]);
  }
  const open = season !== 'winter';
  const size = season === 'spring' ? 0.05 : 0.026;
  for (const [from, to, count] of racemes) {
    const dir = to.clone().sub(from).normalize();
    // Stars from the raceme's foot to three quarters up, turning round it; buds above them.
    if (open) {
      for (let i = 0; i < count; i++) {
        const turn = i * GOLDEN * 2 + rand();
        const out = v3(Math.cos(turn), Math.sin(turn), 0);
        const at = from.clone().lerp(to, (0.75 * (i + 0.5)) / count).addScaledVector(out, 0.025);
        const n = out.multiplyScalar(0.8).addScaledVector(dir, 0.6).normalize();
        star(cards, at, n, size * (0.85 + 0.3 * rand()), along.clone().lerp(buds, season === 'spring' ? 0.15 * rand() : 0), rand() * 2);
      }
    }
    const budFrom = from.clone().lerp(to, open ? 0.7 : 0.35);
    for (const side of [v3().crossVectors(dir, UP).normalize(), v3().crossVectors(dir, v3().crossVectors(dir, UP)).normalize()]) {
      const bulge = budFrom.clone().lerp(to, 0.4).addScaledVector(side, 0.016);
      const other = budFrom.clone().lerp(to, 0.4).addScaledVector(side, -0.016);
      cards.add([budFrom, bulge, to], [along.clone().lerp(buds, 0.5), buds, buds], light);
      cards.add([budFrom, to, other], [along.clone().lerp(buds, 0.5), buds, buds], light);
    }
  }
  parts.push(cards.build());
  return merge(parts);
}

export default {
  name: 'Branched asphodel',
  latin: 'Asphodelus ramosus',
  build: (season, seed) => asphodel(season, random(seed)),
  ground: { on: ['grass', 'bare'], height: [0.5, 1.0], share: 0.05, own: true, clumped: true },
} satisfies Plant;
