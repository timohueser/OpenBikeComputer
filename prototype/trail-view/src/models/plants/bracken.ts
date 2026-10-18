import * as THREE from 'three';
import type { Plant } from '../library';
import { hex, merge, paint, ramp, random, ribbon, sheet, smooth, v3, type Ramp, type Vec } from '../shape';
import type { Season } from '../trees';

/** Bracken: fresh green, rust-orange as it dies in autumn, faded russet through winter. */
const BRACKEN: Record<Season, Ramp> = {
  winter: ['#55301e', '#8a4e2e', '#b27a4c'],
  spring: ['#3f6a24', '#72a43a', '#b0d266'],
  summer: ['#2f5a20', '#5a9030', '#9ac24c'],
  autumn: ['#70300f', '#b8581c', '#e69a44'],
};

/**
 * How far bracken has died back, for every other frond and the ones between: up to 0.35 the blade
 * withers and hangs, beyond that the stalk bends over. The standing fronds keep a clump's height,
 * so it keeps its size through the seasons. Spring's fallen fronds are last year's.
 */
const BRACKEN_SLUMP: Record<Season, [number, number]> = { winter: [0.35, 1], spring: [0, 1], summer: [0, 0], autumn: [0.08, 0.22] };

const PINNAE = 6;

/**
 * A bracken frond: a short stalk curving out of the ground into a triangular blade that rises at 35
 * to 50 degrees and arches over, its pinnae swept forward. `slump` kills it, as BRACKEN_SLUMP says,
 * until a fallen blade props on its tip near the root; a low `open` curls a young blade into a
 * nodding crozier. 28 triangles.
 */
function frond(root: Vec, a: number, size: number, [deep, mid, tip]: THREE.Color[], slump: number, open: number, rand: () => number) {
  const out = v3(Math.cos(a), Math.sin(a), 0);
  const side = v3(-out.y, out.x, 0);
  const [hang, fall, young] = [smooth(0, 0.35, slump), smooth(0.35, 1, slump), 1 - open];
  const curl = Math.min(1, hang + 2 * young);
  const height = size * (0.22 + 0.08 * rand()) * (1 - 0.3 * fall);
  const lean = (0.25 + 0.25 * rand()) * (1 - 0.7 * young) + 0.3 * fall;
  // The stalk bends from near upright at the root to past `lean` at the elbow, so stalk and blade
  // make one arch rather than a pole and a table.
  const step = (from: THREE.Vector3, tilt: number) => from.clone().addScaledVector(out, 0.5 * height * Math.sin(tilt)).setZ(from.z + 0.5 * height * Math.cos(tilt));
  const bend = step(v3(...root), 0.4 * lean);
  const top = step(bend, 1.6 * lean);
  const stalk = paint(ribbon([root, bend.toArray(), top.toArray()], side.toArray(), () => 0.008, 2), deep.clone().lerp(mid, 0.5));
  const length = size * (0.5 + 0.15 * rand()) * (0.55 + 0.45 * open) * (1 - 0.25 * hang) * (1 - 0.3 * fall);
  const rise = 0.6 + 0.25 * rand() + 1.2 * young + 0.3 * hang - 1.6 * fall;
  // The rachis is a parabola from the elbow to where its tip ends: a live tip near the elbow's
  // height, a withered one hanging near the ground, a fallen one on it.
  const live = top.z + length * (young ? 0.05 : -0.06 + 0.12 * rand());
  const end = live + (0.03 + 0.12 * (1 - fall) - live) * hang;
  const droop = Math.sin(rise) - (end - top.z) / length;
  const rachis = (u: number) =>
    top.clone().addScaledVector(out, length * Math.cos(rise) * u).setZ(Math.max(0.02, top.z + length * (Math.sin(rise) * u - droop * u * u)));
  const cards = sheet();
  // Each pinna is a triangle on most of a rachis segment, its apex out to the side and forward,
  // folded down along its midrib: the two halves light apart and notches open between pinnae, so the
  // blade reads lacy rather than as one smooth panel.
  for (const s of [1, -1]) {
    for (let i = 0; i < PINNAE; i++) {
      const [u0, u1] = [(i + 0.08) / PINNAE, (i + 0.92) / PINNAE];
      const midrib = rachis((u0 + u1) / 2);
      const forward = rachis(u1).sub(rachis(u0)).normalize();
      const reach = 0.42 * length * open ** 1.5 * (1 - 0.45 * hang) * (1 - u0) ** 0.85 * (0.85 + 0.3 * rand());
      // Young and withered pinnae fold down and in along the rachis, so the blade hangs narrow.
      const apex = midrib.clone().addScaledVector(side, s * reach * (1 - 0.4 * curl)).addScaledVector(forward, (0.65 + 0.3 * curl) * reach);
      apex.z = Math.max(0.01, apex.z - (0.1 + 1.1 * curl) * reach);
      const fold = 0.25 * midrib.distanceTo(rachis(u0));
      const [back, front] = [rachis(u0), rachis(u1)].map((p) => p.setZ(p.z - fold));
      const stem = deep.clone().lerp(mid, 0.3 + 0.4 * u0);
      const leaf = mid.clone().lerp(tip, 0.1 + 0.3 * u0 + 0.2 * rand());
      const lit = mid.clone().lerp(tip, 0.4 + 0.4 * u0 + 0.2 * rand());
      for (const edge of [back, front]) {
        const n = edge.clone().sub(midrib).cross(apex.clone().sub(midrib)).normalize();
        if (n.z < 0) n.negate();
        cards.add([midrib, edge, apex], [stem, leaf, lit], () => n);
      }
    }
  }
  return merge([stalk, cards.build()]);
}

/**
 * Three to six bracken fronds arching out of a small patch, their blades in tiers. In spring young
 * fronds stand among last year's, propped on the ground. A clump is the same in every season, only
 * older. 84 to 168 triangles.
 */
function bracken(season: Season, rand: () => number): THREE.BufferGeometry {
  const count = 6 - Math.floor(4 * rand());
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < count; k++) {
    const fallen = k % 2 === 1;
    const old = season === 'spring' && fallen;
    const b = (k / count) * 2 * Math.PI + rand() * 0.8;
    const d = 0.05 + 0.2 * rand();
    const colours = ramp(BRACKEN[old ? 'winter' : season]);
    // Autumn fronds die unevenly: some still orange, some already brown.
    const brown = 0.6 * rand();
    const shade = 0.85 + 0.3 * rand();
    colours.forEach((c, i) => c.lerp(hex(BRACKEN.winter[i]), season === 'autumn' ? brown : 0).multiplyScalar(shade));
    const slump = Math.min(1, BRACKEN_SLUMP[season][fallen ? 1 : 0] * (0.85 + 0.3 * rand()));
    const open = season === 'spring' && !old ? 0.55 : 1;
    // Lower fronds around a tall one hide its stalk under their blades, in tiers.
    const size = 1 - (0.55 * k * (0.8 + 0.4 * rand())) / count;
    parts.push(frond([d * Math.cos(b), d * Math.sin(b), 0], b + (rand() - 0.5) * 0.6, size, colours, slump, open, rand));
  }
  return merge(parts);
}

export default {
  name: 'Bracken',
  latin: 'Pteridium aquilinum',
  build: (season, seed) => bracken(season, random(seed)),
  ground: { on: ['grass', 'shrub'], height: [0.6, 1.1], share: 0.12, habitat: 'lush', own: true, clumped: true },
} satisfies Plant;
