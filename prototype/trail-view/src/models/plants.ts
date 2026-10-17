/**
 * Ground plants, 1 tall with their root at the origin and z up, at one level of detail: cheap
 * enough to instance in thousands within 60 m.
 */

import * as THREE from 'three';
import { hash, hex, lump, merge, paint, place, random, ribbon, smooth, soft, tube, unit, v3, type Vec } from './shape';
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

/**
 * A lumpy mound with upright shoots that break its outline. In bloom the shoots and the mound
 * carry the flowers, otherwise rusty tips.
 */
function heather(season: Season, bloom: boolean, seed: number, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(HEATHER[season]);
  const [flower, share] = bloom ? HEATHER_BLOOM[season] : ['#7d6a44', 0.15];
  const dots = hex(flower);
  const foliage = (p: THREE.Vector3, n: THREE.Vector3) => {
    const c = deep.clone().lerp(mid, smooth(0, 0.5, p.z)).lerp(tip, 0.5 * smooth(0.2, 0.9, n.z));
    return hash(p, seed) < share ? c.lerp(dots, 0.5 + 0.5 * smooth(-0.3, 0.4, n.z)) : c;
  };
  const shoot = (p: THREE.Vector3) => mid.clone().lerp(dots, (bloom ? 1.4 : 0.8) * share * smooth(-0.05, 0.1, p.z));
  // The mound's widest ring sits at the ground, so it grows out of it instead of resting on a rim.
  const parts = [paint(place(soft(lump(0.65, 1), 0.14, 3.5, seed), { at: [0, 0, 0.08], scale: [1.25, 1.25, 0.8] }), foliage)];
  for (let k = 0; k < 14; k++) {
    const a = rand() * 2 * Math.PI;
    const d = Math.sqrt(rand()) * 0.7;
    const at: Vec = [d * Math.cos(a), d * Math.sin(a), 0.7 - 0.6 * d * d];
    const spike = soft(new THREE.ConeGeometry(0.06, 0.4, 3, 1, true).rotateX(Math.PI / 2));
    parts.push(place(paint(spike, shoot), { at, rot: [(rand() - 0.5) * 0.6, (rand() - 0.5) * 0.6, rand() * 6] }));
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
};

export type PlantName = keyof typeof PLANTS;

export const plantLabel = (name: PlantName) => PLANTS[name].name;

export const plant = (name: PlantName, season: Season, seed = 1) => unit(PLANTS[name].build(season, seed));
