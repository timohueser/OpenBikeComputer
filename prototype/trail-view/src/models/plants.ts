/**
 * Ground plants, 1 tall with their root at the origin and z up, at one level of detail: cheap
 * enough to instance in thousands within 60 m.
 */

import * as THREE from 'three';
import { hash, hex, lump, merge, paint, place, random, ribbon, smooth, soft, tube, unit, v3, type Vec } from './shape';
import { SUCCULENTS } from './succulents';
import type { Season } from './trees';

/** Shade, body and lit-tip colour. */
type Ramp = [string, string, string];

const GRASS: Record<Season, Ramp> = {
  winter: ['#6e6a4c', '#9a9168', '#c2b88c'],
  spring: ['#4f7a35', '#7fae4a', '#b4d36e'],
  summer: ['#557a34', '#86a84a', '#c2c46a'],
  autumn: ['#6a7038', '#97935a', '#c4b47a'],
};

const FERN: Record<Season, Ramp> = {
  winter: ['#5a4030', '#7a5638', '#94704a'],
  spring: ['#4f8a3a', '#7ab84e', '#b0dc72'],
  summer: ['#2f5e2a', '#4a8236', '#7aab4c'],
  autumn: ['#7a4a22', '#a8682c', '#cf9448'],
};

const HEATHER: Record<Season, Ramp> = {
  winter: ['#3a3a26', '#5a5434', '#7a6a40'],
  spring: ['#34462b', '#5b7340', '#7d9150'],
  summer: ['#34462b', '#5b7340', '#86984f'],
  autumn: ['#3a4228', '#5f6838', '#8c6b40'],
};

/** Flowers in bloom on a clump of bell heather and ling, by season: colour and share. */
const HEATHER_BLOOM: Record<Season, [string, number]> = {
  winter: ['#7a5a3c', 0.3],
  spring: ['#7d9150', 0],
  summer: ['#9a5a8c', 0.8],
  autumn: ['#8a5a62', 0.5],
};

const HUCKLEBERRY: Record<Season, Ramp> = {
  // Bare red twigs: the leaves take the twigs' colour.
  winter: ['#5a3028', '#7a4434', '#8e5a48'],
  spring: ['#4f8a3a', '#7ab84e', '#b0dc72'],
  summer: ['#3a6a2e', '#56883a', '#80aa50'],
  autumn: ['#7a1e22', '#a83228', '#d4583a'],
};

/** Kermes oak, lentisc and rosemary: evergreen, a little duller in winter. */
const GARRIGUE: Record<Season, Ramp> = {
  winter: ['#2a3322', '#41502f', '#66744a'],
  spring: ['#26381e', '#41602c', '#7a9a48'],
  summer: ['#24331d', '#3b5229', '#6a8040'],
  autumn: ['#283420', '#3f5229', '#6c7c42'],
};

/** Esparto blades: grey-green at the root, pale straw at the tip. */
const ESPARTO: Record<Season, Ramp> = {
  winter: ['#66654a', '#9a9672', '#c4b98e'],
  spring: ['#5f6a40', '#98a06a', '#d2c88e'],
  summer: ['#6b6a45', '#a39d6a', '#d8c98e'],
  autumn: ['#6b6a45', '#a39d6a', '#d4c58e'],
};

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

/** Cotton-grass leaves: green with russet tips, crimson in autumn, straw in winter. */
const SEDGE: Record<Season, Ramp> = {
  winter: ['#56442e', '#86704a', '#ac966a'],
  spring: ['#3c5a2a', '#66883c', '#a2a456'],
  summer: ['#3a5828', '#62823a', '#a08a4a'],
  autumn: ['#52241a', '#8a3424', '#bc5a34'],
};

/** Purple moor grass and deer grass: the straw of dead blades, tawny to orange-brown in autumn. */
const MOOR_GRASS: Record<Season, Ramp> = {
  winter: ['#6a4e32', '#a8845a', '#d4b88a'],
  spring: ['#5e4a30', '#9a7e56', '#c8ac7e'],
  summer: ['#5a4e30', '#968458', '#d2bc88'],
  autumn: ['#6a3a1e', '#a8683a', '#c99a5a'],
};

const MOOR_GREEN: Ramp = ['#3a4a1f', '#7a8c3e', '#a2ac58'];

/** Share of a tussock's blades still green, counted from the crown out. */
const MOOR_LIVE: Record<Season, number> = { winter: 0, spring: 0.3, summer: 0.95, autumn: 0.12 };

/** Gentian, buttercup, edelweiss white, aster and pink, as the season has them. */
const FLOWERS: Record<Season, string[]> = {
  winter: ['#8a7456', '#9a8462'],
  spring: ['#2f5fd0', '#f2c832', '#f6f2e6', '#2f5fd0'],
  summer: ['#2f5fd0', '#f2c832', '#f6f2e6', '#9a6ad0', '#e48ab0'],
  autumn: ['#9a6ad0', '#8a7456', '#a08a5a'],
};

const ramp = (r: Ramp) => r.map(hex) as [THREE.Color, THREE.Color, THREE.Color];

/** Darker at the root, lit at the tip, by height. */
const byHeight = ([deep, mid, tip]: THREE.Color[], top: number) => (p: THREE.Vector3) =>
  deep.clone().lerp(mid, smooth(0, 0.4 * top, p.z)).lerp(tip, smooth(0.5 * top, top, p.z));

/**
 * A leaf or blade from `base` towards angle `a`. It leaves at `tilt` from vertical, and its tip
 * drops `arch` lengths below that line.
 */
function blade(base: Vec, a: number, length: number, tilt: number, arch: number, width: (t: number) => number, segments: number) {
  const out: Vec = [Math.cos(a), Math.sin(a), 0];
  const along = (s: number): Vec => [
    base[0] + out[0] * Math.sin(tilt) * s * length,
    base[1] + out[1] * Math.sin(tilt) * s * length,
    base[2] + (Math.cos(tilt) * s - arch * s * s) * length,
  ];
  return ribbon([base, along(0.5), along(1)], [-out[1], out[0], 0], width, segments);
}

/** Nine tapering blades, 27 triangles. */
function grass(season: Season, rand: () => number): THREE.BufferGeometry {
  const colour = byHeight(ramp(GRASS[season]), 0.9);
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 9; k++) {
    const a = (k / 9) * 2 * Math.PI + rand() * 0.6;
    const base: Vec = [0.05 * Math.cos(a), 0.05 * Math.sin(a), 0];
    const geo = blade(base, a, 0.75 + 0.25 * rand(), 0.2 + 0.45 * rand(), 0.35, (t) => 0.1 * (1 - t), 2);
    parts.push(paint(geo, colour));
  }
  return merge(parts);
}

/** A flat five-petalled star facing up, its eye in the middle: ten triangles. */
function star(r: number, petal: THREE.Color, eye: THREE.Color): THREE.BufferGeometry {
  const geo = new THREE.CircleGeometry(r, 10);
  const pos = geo.getAttribute('position');
  for (let i = 2; i < pos.count; i += 2) pos.setXYZ(i, pos.getX(i) * 0.5, pos.getY(i) * 0.5, 0);
  return paint(soft(geo), (p) => eye.clone().lerp(petal, smooth(0.1 * r, 0.5 * r, Math.hypot(p.x, p.y))));
}

/** A low rosette with seven flowers on stalks. */
function flowers(season: Season, rand: () => number): THREE.BufferGeometry {
  const leaf = byHeight(ramp(GRASS.summer), 0.4);
  const stalk = hex(season === 'winter' || season === 'autumn' ? '#8a7a52' : '#5c8a45');
  const colours = FLOWERS[season];
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 5; k++) {
    const a = (k / 5) * 2 * Math.PI + rand();
    parts.push(paint(blade([0, 0, 0], a, 0.45, 1.1, 0.35, (t) => 0.18 * Math.sin(Math.PI * Math.min(1, t + 0.15)), 2), leaf));
  }
  for (let k = 0; k < 7; k++) {
    const a = (k / 7) * 2 * Math.PI + rand() * 0.8;
    const d = 0.15 + 0.3 * rand();
    const h = 0.45 + 0.55 * rand();
    const top: Vec = [d * Math.cos(a), d * Math.sin(a), h];
    const middle: Vec = [0.4 * top[0], 0.4 * top[1], 0.55 * h];
    parts.push(paint(ribbon([[0, 0, 0], middle, top], [-Math.sin(a), Math.cos(a), 0], () => 0.03, 2), stalk));
    const petal = hex(colours[Math.floor(rand() * colours.length)]);
    const head = star(0.14, petal, petal.clone().lerp(hex('#f6e27a'), 0.7));
    parts.push(place(head, { at: top, rot: [(rand() - 0.5) * 0.6, (rand() - 0.5) * 0.6, rand() * 6] }));
  }
  return merge(parts);
}

/** Nine fronds rising out of the crown like a shuttlecock, their tips nodding; the edge steps in and out like the pinnae. */
function fern(season: Season, rand: () => number): THREE.BufferGeometry {
  const colour = byHeight(ramp(FERN[season]), 0.7);
  const width = (t: number) => 0.45 * smooth(0.02, 0.35, t) * (1 - t) ** 0.6 * (Math.round(t * 6) % 2 ? 0.6 : 1);
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 9; k++) {
    const a = (k / 9) * 2 * Math.PI + rand() * 0.5;
    parts.push(paint(blade([0, 0, 0], a, 1.1 + 0.3 * rand(), 0.3 + 0.35 * rand(), 0.5, width, 6), colour));
  }
  return merge(parts);
}

/** A tuft of a cushion: centre x and y, radius and height. */
type Tuft = [number, number, number, number];

/** A tall tuft in the middle and four lower ones around it: a cushion about twice as wide as tall. */
function tufts(rand: () => number): Tuft[] {
  const out: Tuft[] = [[0, 0, 0.52, 1]];
  for (let k = 0; k < 4; k++) {
    const a = (k / 4) * 2 * Math.PI + rand() * 0.8;
    const d = 0.4 + 0.12 * rand();
    out.push([d * Math.cos(a), d * Math.sin(a), 0.38 + 0.1 * rand(), 0.6 + 0.2 * rand()]);
  }
  return out;
}

const GOLDEN = Math.PI * (3 - Math.sqrt(5));

/**
 * Loose triangles whose normals lie on their tuft's dome, leaning up: many thin twigs light as a
 * few round tufts.
 */
function cushion() {
  const position: number[] = [];
  const normal: number[] = [];
  const color: number[] = [];
  return {
    add([x, y, r, h]: Tuft, corners: THREE.Vector3[], colours: THREE.Color[]) {
      corners.forEach((p, i) => {
        const n = v3((p.x - x) / r, (p.y - y) / r, p.z / h).normalize().lerp(v3(0, 0, 1), 0.3).normalize();
        position.push(p.x, p.y, p.z);
        normal.push(n.x, n.y, n.z);
        colours[i].toArray(color, color.length);
      });
    },
    /** A low six-sided cone that fills the gaps between a tuft's sprays with shade. */
    core(tuft: Tuft, shade: THREE.Color, top: THREE.Color, size: number, height: number) {
      const [x, y, r, h] = tuft;
      const ring = (s: number) => v3(x + size * r * Math.cos((s * Math.PI) / 3), y + size * r * Math.sin((s * Math.PI) / 3), -0.02);
      for (let s = 0; s < 6; s++) this.add(tuft, [ring(s), ring(s + 1), v3(x, y, height * h)], [shade, shade, top]);
    },
    build() {
      return new THREE.BufferGeometry()
        .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
        .setAttribute('normal', new THREE.Float32BufferAttribute(normal, 3))
        .setAttribute('color', new THREE.Float32BufferAttribute(color, 3));
    },
  };
}

/**
 * Five round tufts of upright twig sprays around a dark core. Each spray is one triangle from a
 * narrow foot to a ragged top on its tuft's dome. Flowers, or out of bloom rusty tips, colour the
 * top corners, the highest most.
 */
function heather(season: Season, bloom: boolean, seed: number, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(HEATHER[season]);
  const [flower, share] = bloom ? HEATHER_BLOOM[season] : ['#7d6a44', 0.15];
  const [dots, bronze, shade] = [hex(flower), hex('#6a5530'), deep.clone().multiplyScalar(0.7)];
  const cards = cushion();
  for (const tuft of tufts(rand)) {
    const [x, y, r, h] = tuft;
    cards.core(tuft, shade, deep, 0.75, 0.7);
    for (let j = 0; j < 40; j++) {
      const a = j * GOLDEN + rand() * 0.6;
      const polar = 1.5 * Math.sqrt((j + 0.5) / 40);
      const k = 0.9 + 0.15 * rand();
      const top = v3(x + r * k * Math.sin(polar) * Math.cos(a), y + r * k * Math.sin(polar) * Math.sin(a), h * k * Math.cos(polar) ** 0.4);
      // A spray leans out with the dome, but stands more upright than the dome's normal.
      const lean = 0.3 * polar;
      const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -h * (0.45 + 0.2 * rand()));
      foot.z = Math.max(foot.z, 0);
      const turn = a + Math.PI / 2 + (rand() - 0.5) * 0.8;
      const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(0.075 * h * (0.8 + 0.4 * rand()));
      const twig = mid
        .clone()
        .lerp(tip, 0.2 + 0.3 * rand())
        .lerp(bronze, rand() < 0.35 ? 0.5 * rand() : 0)
        .lerp(deep, 0.35 * (1 - smooth(0.2, 0.9, top.z / h)));
      const corners = [top.clone().add(side), top.clone().sub(side)];
      for (const c of corners) c.z -= 0.08 * h * rand();
      const tips = corners.map((c) => (hash(c, seed) < share ? twig.clone().lerp(dots, (bloom ? 1 : 0.6) * (0.2 + 0.8 * smooth(0.4, 0.95, c.z / h))) : twig));
      cards.add(tuft, [foot, ...corners], [shade.clone().lerp(mid, 0.2 * rand()), ...tips]);
    }
  }
  return cards.build();
}

/**
 * Kermes oak, lentisc and rosemary in one evergreen cushion: five leafy tufts around dark cores,
 * a few of them lit by the sun. Each leaf is a kite from a foot inside its tuft out to the dome.
 */
function garrigue(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(GARRIGUE[season]);
  const shade = deep.clone().multiplyScalar(0.7);
  const cards = cushion();
  for (const tuft of tufts(rand)) {
    const [x, y, r, h] = tuft;
    const sun = rand() < 0.3 ? 0.6 : 0.2 * rand();
    cards.core(tuft, deep, deep, 0.72, 0.8);
    for (let j = 0; j < 16; j++) {
      const a = j * GOLDEN + rand() * 0.6;
      const polar = 1.55 * Math.sqrt((j + 0.5) / 16);
      const k = 0.92 + 0.16 * rand();
      const top = v3(x + r * k * Math.sin(polar) * Math.cos(a), y + r * k * Math.sin(polar) * Math.sin(a), h * k * Math.cos(polar) ** 0.6);
      // Leaves lie out along the dome like scales; the lowest hang down over the core.
      const lean = 1.1 * polar;
      const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -0.38 * h);
      foot.z = Math.max(foot.z, 0);
      const middle = foot.clone().lerp(top, 0.7);
      const turn = a + Math.PI / 2 + (rand() - 0.5) * 0.8;
      const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(0.22 * h * (0.8 + 0.4 * rand()));
      const leaf = mid.clone().lerp(tip, sun + 0.2 * rand()).lerp(deep, 0.4 * (1 - smooth(0.2, 0.9, top.z / h)));
      const lit = leaf.clone().lerp(tip, 0.5);
      cards.add(tuft, [foot, middle.clone().add(side), top, foot, top, middle.clone().sub(side)], [shade, leaf, lit, shade, lit, leaf]);
    }
  }
  return cards.build();
}

/** Esparto: a tussock of long wiry blades arching out of the crown, grey-green below, straw at the tips. */
function esparto(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(ESPARTO[season]);
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 26; k++) {
    const a = k * GOLDEN + rand() * 0.4;
    // Inner blades stand, outer ones bow out; no tip reaches the ground.
    const tilt = 0.1 + 0.8 * Math.sqrt(k / 26) * (0.8 + 0.2 * rand());
    const arch = Math.min(0.7 * tilt * (0.8 + 0.4 * rand()), Math.cos(tilt) - 0.1);
    const ends = tip.clone().lerp(mid, 0.4 * rand());
    const colour = (p: THREE.Vector3) => deep.clone().lerp(mid, smooth(0, 0.35, p.length())).lerp(ends, smooth(0.4, 0.95, p.length()));
    const d = 0.04 + 0.08 * Math.sqrt(k / 26);
    parts.push(paint(blade([d * Math.cos(a), d * Math.sin(a), 0], a, 0.9 + 0.4 * rand(), tilt, arch, (t) => 0.06 * (1 - t), 2), colour));
  }
  return merge(parts);
}

/** Four red stems set with small leaves and, in late summer, berries. Winter leaves it bare. */
function huckleberry(season: Season, rand: () => number): THREE.BufferGeometry {
  const leaf = byHeight(ramp(HUCKLEBERRY[season]), 1);
  const [stem, berry] = [hex('#8a4434'), hex('#3c2a6a')];
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 4; k++) {
    const a = (k / 4) * 2 * Math.PI + rand() * 0.8;
    const [c, s] = [Math.cos(a), Math.sin(a)];
    const path = new THREE.CatmullRomCurve3([v3(), v3(0.12 * c, 0.12 * s, 0.4), v3(0.3 * c, 0.3 * s, 0.75 + 0.2 * rand())]);
    parts.push(paint(soft(tube(path.points.map((p) => p.toArray()), 0.025, 0.4, 3, 3)), stem));
    for (let i = 0; i < 7; i++) {
      const at = path.getPointAt(0.3 + 0.1 * i).toArray();
      parts.push(paint(blade(at, rand() * 2 * Math.PI, 0.2, 1.1, 0.2, (t) => 0.13 * Math.sin(Math.PI * t), 2), leaf));
    }
    if (season === 'summer') parts.push(paint(place(soft(new THREE.OctahedronGeometry(0.045)), { at: path.getPointAt(0.6 + 0.1 * rand()) }), berry));
  }
  return merge(parts);
}

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
  const stalk = paint(ribbon([root, bend.toArray(), top.toArray()], side.toArray(), () => 0.008, 2), deep.clone().lerp(mid, 0.5)).toNonIndexed();
  const length = size * (0.5 + 0.15 * rand()) * (0.55 + 0.45 * open) * (1 - 0.25 * hang) * (1 - 0.3 * fall);
  const rise = 0.6 + 0.25 * rand() + 1.2 * young + 0.3 * hang - 1.6 * fall;
  // The rachis is a parabola from the elbow to where its tip ends: a live tip near the elbow's
  // height, a withered one hanging near the ground, a fallen one on it.
  const live = top.z + length * (young ? 0.05 : -0.06 + 0.12 * rand());
  const end = live + (0.03 + 0.12 * (1 - fall) - live) * hang;
  const droop = Math.sin(rise) - (end - top.z) / length;
  const rachis = (u: number) =>
    top.clone().addScaledVector(out, length * Math.cos(rise) * u).setZ(Math.max(0.02, top.z + length * (Math.sin(rise) * u - droop * u * u)));
  const position: number[] = [];
  const normal: number[] = [];
  const color: number[] = [];
  const face = (corners: THREE.Vector3[], colours: THREE.Color[], n: THREE.Vector3) =>
    corners.forEach((p, i) => {
      position.push(p.x, p.y, p.z);
      normal.push(n.x, n.y, n.z);
      colours[i].toArray(color, color.length);
    });
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
        face([midrib, edge, apex], [stem, leaf, lit], n.z < 0 ? n.negate() : n);
      }
    }
  }
  const blade = new THREE.BufferGeometry()
    .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
    .setAttribute('normal', new THREE.Float32BufferAttribute(normal, 3))
    .setAttribute('color', new THREE.Float32BufferAttribute(color, 3));
  return merge([stalk, blade]);
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

/**
 * A head on a stalk tip, its foot on the tip and its length along `axis`: a soft ball with its
 * corners pushed out a little.
 */
function head(ball: THREE.BufferGeometry, tip: THREE.Vector3, axis: THREE.Vector3, stretch: number, low: THREE.Color, high: THREE.Color, rand: () => number) {
  const geo = soft(ball);
  const pos = geo.getAttribute('position');
  const nrm = geo.getAttribute('normal');
  const p = v3();
  for (let i = 0; i < pos.count; i++) pos.setXYZ(i, ...p.fromBufferAttribute(pos, i).multiplyScalar(1 + 0.15 * rand()).toArray());
  // Normals lean up, so the head lights as one soft tuft rather than a cut stone.
  for (let i = 0; i < nrm.count; i++) nrm.setXYZ(i, ...p.fromBufferAttribute(pos, i).normalize().lerp(v3(0, 0, 1), 0.5).normalize().toArray());
  geo.rotateZ(rand() * 2 * Math.PI).scale(1, 1, stretch);
  geo.computeBoundingBox();
  const { min, max } = geo.boundingBox!;
  place(geo, { at: tip.clone().addScaledVector(axis, -0.85 * min.z), quat: new THREE.Quaternion().setFromUnitVectors(v3(0, 0, 1), axis) });
  return paint(geo, (q) => low.clone().lerp(high, smooth(tip.z + 0.3 * min.z, tip.z + 0.8 * max.z, q.z)));
}

/**
 * A cotton-grass tuft: seven narrow arching leaves and seven thin stalks, each topped in summer by
 * a white cotton tuft that nods off it and in spring by a dark flower spikelet. Out of flower the
 * stalks taper, lean and break like dry stems. 175 triangles in summer, 91 in spring, 42 without
 * heads.
 */
function bogCotton(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(SEDGE[season]);
  const leaf = (p: THREE.Vector3) => deep.clone().lerp(mid, smooth(0, 0.2, p.length())).lerp(tip, smooth(0.25, 0.55, p.length()));
  const dead = season === 'autumn' || season === 'winter';
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 7; k++) {
    const a = k * GOLDEN + rand() * 0.5;
    parts.push(paint(blade([0.03 * Math.cos(a), 0.03 * Math.sin(a), 0], a, 0.45 + 0.2 * rand(), 0.25 + 0.5 * rand(), dead ? 0.6 : 0.4, (t) => 0.07 * (1 - t), 2), leaf));
  }
  const stalk = hex(dead ? '#8a6e48' : '#6e7a48');
  for (let k = 0; k < 7; k++) {
    const a = k * GOLDEN + 1 + rand() * 0.6;
    const [c, s] = [Math.cos(a), Math.sin(a)];
    const d = 0.08 + 0.22 * rand();
    const foot: Vec = [0.02 * c, 0.02 * s, 0];
    const top = v3(d * c, d * s, 0.75 + 0.25 * rand());
    if (!dead) {
      parts.push(paint(ribbon([foot, top.toArray()], [-s, c, 0], (t) => 0.016 * (1 - 0.4 * t), 1), stalk));
      const up = top.clone().sub(v3(...foot)).normalize();
      if (season === 'spring') {
        parts.push(head(new THREE.OctahedronGeometry(0.026), top, up, 1.5, hex('#3e3226'), hex('#6a5a40'), rand));
        continue;
      }
      // The cotton nods out, away from the tuft.
      const tilt = 0.5 + 0.5 * rand();
      const nod = up.clone().multiplyScalar(Math.cos(tilt)).add(v3(c, s, 0).multiplyScalar(Math.sin(tilt))).normalize();
      parts.push(head(lump(0.04, 0), top, nod, 1.4, hex('#e2ded2'), hex('#ffffff'), rand));
      continue;
    }
    // Dry stems: one in three broken over at the kink, one in three shorter.
    const broken = k % 3 === 1;
    if (k % 3 === 2) top.z *= 0.7;
    // At half the stem, where the ribbon puts its middle vertex.
    const kink = v3(...foot).lerp(top, 0.5);
    const bend = broken ? 0.7 + 0.6 * rand() : 0.15 * rand();
    const rest = top.distanceTo(kink);
    const over = v3(c, s, 0).multiplyScalar(Math.sin(bend)).add(top.clone().sub(kink).normalize().multiplyScalar(Math.cos(bend))).normalize();
    const end = kink.clone().addScaledVector(over, rest);
    parts.push(paint(ribbon([foot, kink.toArray(), end.toArray()], [-s, c, 0], (t) => 0.018 * (1 - t), 2), stalk));
  }
  return merge(parts);
}

/**
 * A tussock of purple moor grass or deer grass, lit as one dome: blades taper from inside it to
 * points on a ragged top, and a fringe of long blades rises out of its edge and bows over nearly
 * to the ground. Live blades grow from the crown out as the season has them, dead straw beyond. 98
 * triangles.
 */
function moorGrass(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(MOOR_GRASS[season]);
  const [greenDeep, greenMid] = ramp(MOOR_GREEN);
  // Live blades go buff at the tip only as they turn in autumn.
  const buff = season === 'autumn' ? 0.35 : 0.1;
  const tuft: Tuft = [0, 0, 0.6, 1];
  const cards = cushion();
  // Root, blade and tip: a live blade is green, a dead one straw throughout. The root's shade
  // fades out by a third of the height, so a long front blade is no dark stake.
  const colours = (live: boolean, foot: number) => {
    const [r1, r2] = [rand(), rand()];
    const body = live ? greenMid.clone().lerp(tip, 0.5 * buff + 0.15 * r1) : mid.clone().lerp(tip, 0.35 + 0.4 * r1);
    const end = live ? greenMid.clone().lerp(tip, buff + 0.35 * buff * r2 + 0.15 * r2) : body.clone().lerp(tip, 0.3);
    const root = (live ? greenDeep.clone().lerp(greenMid, 0.2) : deep.clone().lerp(mid, 0.2)).lerp(body, 0.35 + 0.65 * smooth(0, 0.3, foot));
    return [root, body, end];
  };
  const across = (a: number, width: number) => {
    const turn = a + Math.PI / 2 + (rand() - 0.5) * 0.8;
    return v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(width * (0.8 + 0.4 * rand()));
  };
  const dome = 54;
  for (let j = 0; j < dome; j++) {
    const a = j * GOLDEN + rand() * 0.5;
    const polar = 1.25 * Math.sqrt((j + 0.5) / dome);
    const k = 0.88 + 0.22 * rand();
    const top = v3(0.6 * k * Math.sin(polar) * Math.cos(a), 0.6 * k * Math.sin(polar) * Math.sin(a), k * Math.cos(polar) ** 0.6 + 0.1 * rand());
    // A blade stands more upright than the dome's normal.
    const lean = 0.6 * polar;
    const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -0.6 - 0.25 * rand());
    foot.z = Math.max(foot.z, 0);
    const side = across(a, 0.04);
    const [root, body, end] = colours(j < MOOR_LIVE[season] * dome, foot.z);
    cards.add(tuft, [foot.clone().add(side), foot.clone().sub(side), top], [root, body, end]);
  }
  const fringe = 22;
  for (let j = 0; j < fringe; j++) {
    const a = ((j + rand() * 0.8) / fringe) * 2 * Math.PI;
    const polar = 0.9 + 0.25 * rand();
    const foot = v3(0.5 * Math.sin(polar) * Math.cos(a), 0.5 * Math.sin(polar) * Math.sin(a), 0.9 * Math.cos(polar) ** 0.6);
    const reach = 0.85 + 0.2 * rand();
    const out = a + (rand() - 0.5) * 0.3;
    const end = v3(reach * Math.cos(out), reach * Math.sin(out), 0.08 + 0.17 * rand());
    // A kite bent at its widest and bowed up there, so the blade arches out of the tussock's side
    // and over toward the ground in one curve.
    const bend = foot.clone().lerp(end, 0.45);
    bend.z += 0.07 + 0.05 * rand();
    const side = across(out, 0.035);
    const [root, body, tipColour] = colours(rand() < MOOR_LIVE[season], foot.z);
    cards.add(tuft, [foot, bend.clone().add(side), bend.clone().sub(side), bend.clone().add(side), end, bend.clone().sub(side)], [root, body, body, body, tipColour, body]);
  }
  return cards.build();
}

const PLANTS = {
  grass: { name: 'Grass tuft', build: (season: Season, seed: number) => grass(season, random(seed)) },
  'alpine-flowers': { name: 'Alpine flowers', build: (season: Season, seed: number) => flowers(season, random(seed)) },
  fern: { name: 'Fern', build: (season: Season, seed: number) => fern(season, random(seed)) },
  heather: { name: 'Heather', build: (season: Season, seed: number) => heather(season, false, seed, random(seed)) },
  'heather-bloom': { name: 'Heather, flowering', build: (season: Season, seed: number) => heather(season, true, seed, random(seed)) },
  huckleberry: { name: 'Huckleberry', build: (season: Season, seed: number) => huckleberry(season, random(seed)) },
  garrigue: { name: 'Garrigue', build: (season: Season, seed: number) => garrigue(season, random(seed)) },
  esparto: { name: 'Esparto', build: (season: Season, seed: number) => esparto(season, random(seed)) },
  bracken: { name: 'Bracken', build: (season: Season, seed: number) => bracken(season, random(seed)) },
  'bog-cotton': { name: 'Bog cotton', build: (season: Season, seed: number) => bogCotton(season, random(seed)) },
  'moor-grass': { name: 'Moor grass', build: (season: Season, seed: number) => moorGrass(season, random(seed)) },
  ...SUCCULENTS,
};

export type PlantName = keyof typeof PLANTS;

export const plantLabel = (name: PlantName) => PLANTS[name].name;

export const plant = (name: PlantName, season: Season, seed = 1) => unit(PLANTS[name].build(season, seed));
