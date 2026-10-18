/**
 * Shrubs of the Alpine treeline, in the shape of the ground plant table: 1 tall with the root at
 * the origin and z up. A seed draws the same bush in every season; the season changes only its
 * colours, leaves, flowers and berries.
 */

import * as THREE from 'three';
import { hash, hex, merge, paint, random, ribbon, smooth, soft, tube, v3, type Vec } from './shape';
import type { Season } from './trees';

/** Shade, body and lit-tip colour. */
type Ramp = [string, string, string];

const ramp = (r: Ramp) => r.map(hex) as [THREE.Color, THREE.Color, THREE.Color];

const GOLDEN = Math.PI * (3 - Math.sqrt(5));
const UP = v3(0, 0, 1);

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

/** Mountain pine needles: dark green, a little olive in winter. */
const MUGO: Record<Season, Ramp> = {
  winter: ['#1c3024', '#2e4c34', '#5a7a4c'],
  spring: ['#1a3224', '#2c5236', '#62904e'],
  summer: ['#1a3024', '#2c4e36', '#5a884c'],
  autumn: ['#1a3024', '#2c4c34', '#58844a'],
};

/** Pale new shoots on the top brushes: colour and share, by season. */
const MUGO_CANDLES: Record<Season, [string, number]> = {
  winter: ['#4c6a42', 0],
  spring: ['#a8b878', 0.5],
  summer: ['#7a9a5a', 0.3],
  autumn: ['#4c6a42', 0],
};

/** Loose triangles, each corner lit with the normal `light` gives it. */
function sheet() {
  const position: number[] = [];
  const normal: number[] = [];
  const color: number[] = [];
  return {
    add(corners: THREE.Vector3[], colours: THREE.Color[], light: (p: THREE.Vector3) => THREE.Vector3) {
      corners.forEach((p, i) => {
        const n = light(p);
        position.push(p.x, p.y, p.z);
        normal.push(n.x, n.y, n.z);
        colours[i].toArray(color, color.length);
      });
    },
    build: () =>
      new THREE.BufferGeometry()
        .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
        .setAttribute('normal', new THREE.Float32BufferAttribute(normal, 3))
        .setAttribute('color', new THREE.Float32BufferAttribute(color, 3)),
  };
}

/** Normals out from a dome around `centre` with half-sizes `size`, leaning `lift` of the way up. */
const domed = (centre: THREE.Vector3, size: THREE.Vector3, lift: number) => (p: THREE.Vector3) =>
  p.clone().sub(centre).divide(size).normalize().lerp(UP, lift).normalize();

/** A tuft of a cushion: centre x and y, radius and height. */
type Tuft = [number, number, number, number];

/** `middle` and `count` lower tufts round it, about `d` out. */
function tufts(middle: Tuft, count: number, d: number, rand: () => number): Tuft[] {
  const out = [middle];
  for (let k = 0; k < count; k++) {
    const a = (k / count) * 2 * Math.PI + rand() * 0.7;
    const at = d + 0.1 * rand();
    out.push([at * Math.cos(a), at * Math.sin(a), 0.42 + 0.08 * rand(), 0.55 + 0.2 * rand()]);
  }
  return out;
}

/** A low six-sided cone that fills the middle of a tuft with shade. */
function core(cards: ReturnType<typeof sheet>, [x, y, r, h]: Tuft, foot: THREE.Color, top: THREE.Color) {
  const light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
  const ring = (s: number) => v3(x + 0.75 * r * Math.cos((s * Math.PI) / 3), y + 0.75 * r * Math.sin((s * Math.PI) / 3), -0.02);
  for (let s = 0; s < 6; s++) cards.add([ring(s), ring(s + 1), v3(x, y, 0.8 * h)], [foot, foot, top], light);
}

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
  for (const tuft of tufts([0, 0, 0.55, 1], 4, 0.45, rand)) {
    const [x, y, r, h] = tuft;
    const light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
    core(cards, tuft, deep, deep.clone().lerp(mid, 0.4));
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
    parts.push(paint(ribbon([root.toArray(), end.toArray()], flat, (t) => 0.02 - 0.008 * t, 1), stem).toNonIndexed());
    // The twig forks off at an angle, so the tuft reads as a broom of angular stems.
    const s = k % 2 ? 1 : -1;
    const kink = root.clone().addScaledVector(dir, 0.4 * length);
    const fork = dir.clone().addScaledVector(across, 0.8 * s).normalize();
    const twig = kink.clone().addScaledVector(fork, 0.45 * length);
    parts.push(paint(ribbon([kink.toArray(), twig.toArray()], flat, () => 0.014, 1), stem).toNonIndexed());
    for (const [i, t] of [0.3, 0.45, 0.6, 0.75, 0.9, 1].entries()) {
      const at = root.clone().addScaledVector(dir, t * length);
      const spread = across.clone().multiplyScalar(i % 2 ? s : -s).applyAxisAngle(UP, (rand() - 0.5) * 0.8).addScaledVector(out, 0.3);
      leaf(at, spread);
      if (berries && i === 2) {
        berries--;
        const hang = at.clone().addScaledVector(spread.normalize(), 0.04).setZ(at.z - 0.05);
        const ball = soft(new THREE.TetrahedronGeometry(0.04)).translate(hang.x, hang.y, hang.z);
        parts.push(paint(ball, (p) => berry.clone().lerp(bloom, smooth(hang.z - 0.02, hang.z + 0.03, p.z))).toNonIndexed());
      }
    }
    leaf(kink.clone().lerp(twig, 0.5), across.clone().multiplyScalar(-s).addScaledVector(out, 0.5));
    leaf(twig, fork);
  }
  parts.push(cards.build());
  return merge(parts);
}

/**
 * Mountain pine: six round clumps of upturned needle brushes around dark cores, a low mound about
 * twice as wide as tall, and two crooked limbs that creep out from under it along the ground and
 * turn up into brushes of their own. Each brush is one triangle from a narrow foot to a wide top,
 * standing more upright than the dome it sits on; all light as their clump does.
 */
function mountainPine(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(MUGO[season]);
  const [candle, share] = MUGO_CANDLES[season];
  const shoot = hex(candle);
  const [bark, barkLit] = [hex('#3a302a'), hex('#6e6052')];
  const shade = deep.clone().multiplyScalar(0.75);
  const cards = sheet();
  const parts: THREE.BufferGeometry[] = [];
  // A brush faces out along `heading`; `low` darkens the ones deep in the clump.
  const brush = (foot: THREE.Vector3, top: THREE.Vector3, heading: number, low: number, light: (p: THREE.Vector3) => THREE.Vector3) => {
    const turn = heading + Math.PI / 2 + (rand() - 0.5) * 0.8;
    const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(0.09 * (0.8 + 0.4 * rand()));
    const colour = mid.clone().lerp(tip, 0.15 + 0.45 * rand()).lerp(deep, 0.4 * low);
    if (low < 0.25 && rand() < share) colour.lerp(shoot, 0.6);
    const corners = [top.clone().add(side), top.clone().sub(side)];
    for (const c of corners) c.z -= 0.04 * rand();
    cards.add([foot, ...corners], [shade.clone().lerp(mid, 0.2 * rand()), colour, colour.clone().lerp(deep, 0.25)], light);
  };
  const clumps = tufts([0, 0, 0.5, 0.9], 5, 0.48, rand);
  for (const clump of clumps) {
    const [x, y, r, h] = clump;
    const light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
    core(cards, clump, shade, deep.clone().lerp(mid, 0.3));
    for (let j = 0; j < 35; j++) {
      const a = j * GOLDEN + rand() * 0.6;
      const polar = 1.5 * Math.sqrt((j + 0.5) / 35);
      const k = 0.9 + 0.15 * rand();
      const top = v3(x + r * k * Math.sin(polar) * Math.cos(a), y + r * k * Math.sin(polar) * Math.sin(a), h * k * Math.cos(polar) ** 0.5);
      // Brushes turn up at the branch ends, so they stand more upright than the dome.
      const lean = 0.35 * polar;
      const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -h * (0.28 + 0.12 * rand()));
      foot.z = Math.max(foot.z, 0);
      brush(foot, top, a, 1 - smooth(0.2, 0.9, top.z / h), light);
    }
  }
  for (const [x, y, r] of [clumps[1], clumps[3]]) {
    const d = Math.hypot(x, y);
    const out = v3(x / d, y / d, 0);
    const bend = v3(-out.y, out.x, 0).multiplyScalar((rand() - 0.5) * 0.3);
    const end = out.clone().multiplyScalar(d + 0.9 * r).setZ(0.2);
    const path: Vec[] = [[0.2 * out.x, 0.2 * out.y, 0.03], out.clone().multiplyScalar(0.7 * d).add(bend).setZ(0.04).toArray(), out.clone().multiplyScalar(d + 0.8 * r).add(bend).setZ(0.05).toArray(), end.toArray()];
    parts.push(paint(soft(tube(path, 0.04, 0.6, 3, 3)), (p) => bark.clone().lerp(barkLit, smooth(0, 0.3, p.z))).toNonIndexed());
    const light = domed(end.clone().setZ(0), v3(0.2, 0.2, 0.45), 0.3);
    for (let i = 0; i < 5; i++) {
      const a = (i / 5) * 2 * Math.PI + rand() * 0.6;
      brush(end, end.clone().add(v3(0.08 * Math.cos(a), 0.08 * Math.sin(a), 0.18 + 0.06 * rand())).addScaledVector(out, 0.05), a, 0, light);
    }
  }
  parts.push(cards.build());
  return merge(parts);
}

export const MOUNTAIN = {
  alpenrose: { name: 'Alpenrose', build: (season: Season, seed: number) => alpenrose(season, random(seed)) },
  bilberry: { name: 'Bilberry', build: (season: Season, seed: number) => bilberry(season, random(seed)) },
  'mountain-pine': { name: 'Mountain pine', build: (season: Season, seed: number) => mountainPine(season, random(seed)) },
};
