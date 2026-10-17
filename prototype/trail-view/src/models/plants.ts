/**
 * Ground plants, 1 tall with their root at the origin and z up, at one level of detail: cheap
 * enough to instance in thousands within 60 m.
 */

import * as THREE from 'three';
import { hash, hex, merge, paint, place, random, ribbon, smooth, soft, tube, unit, v3, type Vec } from './shape';
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

/** Nine fronds arching out of the crown; the edge steps in and out like the pinnae. */
function fern(season: Season, rand: () => number): THREE.BufferGeometry {
  const colour = byHeight(ramp(FERN[season]), 0.7);
  const width = (t: number) => 0.45 * smooth(0.02, 0.35, t) * (1 - t) ** 0.6 * (Math.round(t * 6) % 2 ? 0.6 : 1);
  const parts: THREE.BufferGeometry[] = [];
  for (let k = 0; k < 9; k++) {
    const a = (k / 9) * 2 * Math.PI + rand() * 0.5;
    parts.push(paint(blade([0, 0, 0], a, 1.1 + 0.3 * rand(), 0.5 + 0.3 * rand(), 0.55, width, 6), colour));
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

const PLANTS = {
  grass: { name: 'Grass tuft', build: (season: Season, seed: number) => grass(season, random(seed)) },
  'alpine-flowers': { name: 'Alpine flowers', build: (season: Season, seed: number) => flowers(season, random(seed)) },
  fern: { name: 'Fern', build: (season: Season, seed: number) => fern(season, random(seed)) },
  heather: { name: 'Heather', build: (season: Season, seed: number) => heather(season, false, seed, random(seed)) },
  'heather-bloom': { name: 'Heather, flowering', build: (season: Season, seed: number) => heather(season, true, seed, random(seed)) },
  huckleberry: { name: 'Huckleberry', build: (season: Season, seed: number) => huckleberry(season, random(seed)) },
  garrigue: { name: 'Garrigue', build: (season: Season, seed: number) => garrigue(season, random(seed)) },
  esparto: { name: 'Esparto', build: (season: Season, seed: number) => esparto(season, random(seed)) },
};

export type PlantName = keyof typeof PLANTS;

export const plantLabel = (name: PlantName) => PLANTS[name].name;

export const plant = (name: PlantName, season: Season, seed = 1) => unit(PLANTS[name].build(season, seed));
