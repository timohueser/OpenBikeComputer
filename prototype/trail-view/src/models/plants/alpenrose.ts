import * as THREE from 'three';
import type { Plant } from '../library';
import { core, domed, GOLDEN, hash, hex, ramp, random, sheet, smooth, tufts, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Alpenrose leaves: dark glossy evergreen, bronzed by the winter. */
const ALPENROSE: Record<Season, Ramp> = {
  winter: ['#22301e', '#3c4e2e', '#6a7646'],
  spring: ['#1e3420', '#36582e', '#74984e'],
  summer: ['#1e3220', '#34562e', '#6c924c'],
  autumn: ['#20321f', '#36522d', '#6a8a48'],
};

/** Share of alpenrose leaves turned to show their rust-brown undersides. */
const ALPENROSE_RUST: Record<Season, number> = { winter: 0.3, spring: 0.15, summer: 0.12, autumn: 0.2 };

const ALPENROSE_FLOWER: Ramp = ['#a81e40', '#d0305a', '#f27094'];

/**
 * Alpenrose: five round tufts of leaves around dark cores, a cushion about twice as wide as tall.
 * Each leaf is a narrow kite from a foot inside its tuft out to the dome, lying along it like a
 * scale; a few show their rusty undersides or a glossy light. In summer pink-red flower trusses
 * crown the tufts, sunk so the bush keeps its size when scaled to 1 tall.
 */
function alpenrose(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(ALPENROSE[season]);
  const [dark, bloom, lit] = ramp(ALPENROSE_FLOWER);
  const [rust, gloss] = [hex('#7a4a30'), hex('#b4ccaa')];
  const shade = deep.clone().multiplyScalar(0.7);
  const cards = sheet();
  const sites: THREE.Vector3[] = [];
  let crest = 0;
  for (const tuft of tufts([0, 0, 0.55, 1], 4, 0.7, [0.45, 0.42, 0.55], [0.1, 0.08, 0.2], rand)) {
    const [x, y, r, h] = tuft;
    const light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
    core(cards, tuft, deep, deep.clone().lerp(mid, 0.4), 0.75, 0.8);
    const sun = rand() < 0.3 ? 0.5 : 0.2 * rand();
    for (let j = 0; j < 16; j++) {
      const a = j * GOLDEN + rand() * 0.6;
      const polar = 1.55 * Math.sqrt((j + 0.5) / 16);
      const k = 0.92 + 0.16 * rand();
      const top = v3(x + r * k * Math.sin(polar) * Math.cos(a), y + r * k * Math.sin(polar) * Math.sin(a), h * k * Math.cos(polar) ** 0.6);
      // Leaves lie out along the dome like scales; the lowest hang down over the core.
      const lean = 1.1 * polar;
      const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -0.4 * h);
      foot.z = Math.max(foot.z, 0);
      const middle = foot.clone().lerp(top, 0.6);
      const turn = a + Math.PI / 2 + (rand() - 0.5) * 0.8;
      const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(0.2 * h * (0.8 + 0.4 * rand()));
      const low = 1 - smooth(0.2, 0.9, top.z / h);
      const leaf = mid.clone().lerp(tip, sun + 0.2 * rand()).lerp(deep, 0.4 * low);
      const end = leaf.clone().lerp(tip, 0.4);
      const roll = rand();
      if (roll < ALPENROSE_RUST[season] * (0.5 + low)) end.lerp(rust, 0.7);
      else if (roll > 0.85) end.lerp(gloss, 0.5);
      cards.add([foot, middle.clone().add(side), top, foot, top, middle.clone().sub(side)], [shade, leaf, end, shade, end, leaf], light);
      crest = Math.max(crest, top.z);
      if (season === 'summer' && polar < 0.9) sites.push(top);
    }
  }
  // A truss: a rounded six-pointed umbel on a twig tip, lit round its own middle. Nine upper tips
  // are picked by hash rather than rand, so a bush is the same in every season.
  sites.sort((p, q) => hash(p, 7) - hash(q, 7));
  for (const site of sites.slice(0, 9)) {
    const truss = site.clone().setZ(Math.min(site.z - 0.03, crest - 0.12));
    const own = domed(truss.clone().setZ(truss.z - 0.06), v3(1, 1, 1), 0.2);
    const crown = truss.clone().setZ(truss.z + 0.12);
    const spin = hash(site, 3) * Math.PI;
    const umbel = Array.from({ length: 7 }, (_, i) => {
      const t = spin + (i / 6) * 2 * Math.PI;
      const k = 0.14 * (i % 2 ? 0.75 : 1);
      return truss.clone().add(v3(k * Math.cos(t), k * Math.sin(t), i % 2 ? 0.02 : -0.02));
    });
    for (let i = 0; i < 6; i++) cards.add([crown, umbel[i], umbel[i + 1]], [lit, i % 2 ? dark : bloom, i % 2 ? bloom : dark], own);
  }
  return cards.build();
}

export default {
  name: 'Alpenrose',
  latin: 'Rhododendron ferrugineum',
  build: (season, seed) => alpenrose(season, random(seed)),
  ground: { on: ['shrub', 'grass'], height: [0.4, 0.9], share: 0.1, own: true, clumped: true },
} satisfies Plant;
