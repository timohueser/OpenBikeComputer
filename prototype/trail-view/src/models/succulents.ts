/**
 * Plants of dry karst and the Canaries, in the shape of the ground plant table: 1 tall with the
 * root at the origin and z up. They are evergreen, so a season changes little but the flowers.
 */

import * as THREE from 'three';
import { hash, hex, lathe, merge, noise, paint, random, ribbon, smooth, soft, tube, v3, type Vec } from './shape';
import type { Season } from './trees';

/** Shade, body and lit-tip colour. */
type Ramp = [string, string, string];

const ramp = (r: Ramp) => r.map(hex) as [THREE.Color, THREE.Color, THREE.Color];

const GOLDEN = Math.PI * (3 - Math.sqrt(5));
const UP = v3(0, 0, 1);

/** Palmito fans: blue grey-green, dustier at the end of the dry summer. */
const PALM: Record<Season, Ramp> = {
  winter: ['#34503f', '#5e8270', '#9cb698'],
  spring: ['#34543f', '#62876e', '#a4c09a'],
  summer: ['#3c503f', '#6a826c', '#a8b692'],
  autumn: ['#365040', '#62826c', '#a0b494'],
};

/** Cardón stems: glaucous grey-green after the winter rains, yellower in the drought. */
const CARDON: Record<Season, Ramp> = {
  winter: ['#374e3b', '#62845e', '#a0b488'],
  spring: ['#38503c', '#668862', '#a6b88a'],
  summer: ['#465040', '#74825a', '#aaac7c'],
  autumn: ['#3a4f3c', '#68845e', '#a2b286'],
};

/** Red cyathia on the stem tips in spring, red-brown capsules in summer: colour and share. */
const CARDON_TIPS: Record<Season, [string, number]> = {
  winter: ['#a8b46c', 0],
  spring: ['#a8282a', 0.7],
  summer: ['#8a5a34', 0.4],
  autumn: ['#a8b46c', 0],
};

/** Tabaiba leaves: pale glaucous green, yellowing as the summer drought sheds them. */
const TABAIBA: Record<Season, Ramp> = {
  winter: ['#5a7a4a', '#8eb070', '#c4d896'],
  spring: ['#5a7a4a', '#92b272', '#c8da98'],
  summer: ['#7a7a44', '#aaa864', '#d2c88a'],
  autumn: ['#5e7a4a', '#90ae6e', '#c0d292'],
};

/** Silver rosette leaves of the tajinaste. */
const SILVER: Ramp = ['#5e6a5a', '#9aa496', '#cdd2c4'];

/** The spike: coral red in flower (buds, flowers, bract tips), spent by late summer, dry in winter. */
const SPIKE: Record<Season, Ramp> = {
  winter: ['#5a4434', '#7e6048', '#a08262'],
  spring: ['#8e3036', '#d8505a', '#f08a84'],
  summer: ['#6a3a2c', '#9a5a40', '#c08262'],
  autumn: ['#5e4636', '#84644a', '#a68866'],
};

/** Retama twigs: grey olive, browner in winter. */
const RETAMA: Record<Season, Ramp> = {
  winter: ['#454a38', '#767c5c', '#a4a684'],
  spring: ['#434e3a', '#768662', '#a8b48a'],
  summer: ['#434c3a', '#748260', '#a4b086'],
  autumn: ['#434c3a', '#748260', '#a2ae86'],
};

/** Retama flowers in late spring, black pods in summer: colour and share of twig tips. */
const RETAMA_BLOOM: Record<Season, [string, number]> = {
  winter: ['#969a76', 0],
  spring: ['#f8f2ee', 0.85],
  summer: ['#4a4030', 0.25],
  autumn: ['#969a76', 0],
};

/** An indexed mesh with smooth normals from flat position and index arrays. */
function mesh(position: number[], index: number[]): THREE.BufferGeometry {
  const geo = new THREE.BufferGeometry().setAttribute('position', new THREE.Float32BufferAttribute(position, 3));
  geo.setIndex(index);
  geo.computeVertexNormals();
  return geo;
}

/**
 * Points normals out from a dome around `centre` with half-sizes `size`, leaning `lift` of the
 * way up, so many small parts light as one round bush.
 */
function domed(geo: THREE.BufferGeometry, centre: THREE.Vector3, size: THREE.Vector3, lift: number): THREE.BufferGeometry {
  const pos = geo.getAttribute('position');
  const nrm = geo.getAttribute('normal');
  const n = v3();
  for (let i = 0; i < pos.count; i++) {
    n.fromBufferAttribute(pos, i).sub(centre).divide(size).normalize().lerp(UP, lift).normalize();
    nrm.setXYZ(i, n.x, n.y, n.z);
  }
  return geo;
}

/**
 * A stiff fan facing `facing` from its hinge, continuing `along`: `segments` pleated segments
 * over `span` radians, split more than halfway in. Two triangles a segment.
 */
function fan(hinge: THREE.Vector3, facing: THREE.Vector3, along: THREE.Vector3, r: number, segments: number, span: number, rand: () => number) {
  const side = v3().crossVectors(facing, along).normalize();
  const up = v3().crossVectors(side, facing).normalize();
  const at = (a: number, d: number) => {
    const p = hinge.clone().addScaledVector(up, d * Math.cos(a)).addScaledVector(side, d * Math.sin(a)).addScaledVector(facing, (0.12 * d * d) / r);
    // Segment tips bow a little under their own weight.
    p.z -= (0.12 * d * d) / r;
    return p;
  };
  const points = [hinge];
  for (let i = 0; i <= segments; i++) points.push(at(-span / 2 + (i / segments) * span, 0.42 * r));
  for (let i = 0; i < segments; i++) points.push(at(-span / 2 + ((i + 0.5) / segments) * span, r * (0.9 + 0.15 * rand())));
  const index: number[] = [];
  for (let i = 0; i < segments; i++) {
    const tip = segments + 2 + i;
    index.push(0, 1 + i, tip, 0, tip, 2 + i);
  }
  return mesh(points.flatMap((p) => p.toArray()), index);
}

/**
 * Dwarf fan palm: nine stiff fans on short stalks, the inner ones upright, the outer ones held
 * out, round three pale folded spear leaves, and one dead fan hanging straw-brown against the
 * foot. Each fan is striped light along its segments and dark in the folds between. 24 triangles
 * a fan with its stalk, 249 in all.
 */
function fanPalm(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(PALM[season]);
  const [straw, strawLit, stalk] = [hex('#7a6444'), hex('#b09a6e'), hex('#76805a')];
  const parts: THREE.BufferGeometry[] = [];
  const count = 10;
  for (let k = 0; k < count; k++) {
    const a = k * GOLDEN + rand() * 0.5;
    const q = (k + 0.5) / count;
    const dead = k === count - 1;
    const tilt = dead ? 0.3 : 0.08 + 0.85 * Math.sqrt(q);
    const out = v3(Math.cos(a), Math.sin(a), 0);
    const dir = out.clone().multiplyScalar(Math.sin(tilt)).setZ(Math.cos(tilt));
    const foot = out.clone().multiplyScalar(0.04);
    const hinge = foot.clone().addScaledVector(dir, dead ? 0.38 : 0.52 * (1 - 0.3 * q) + 0.08 * rand());
    // Fans face out and up, the outer ones more up; the dead one hangs down the outside of the foot.
    const lift = dead ? 0.46 : 0.3 + 0.45 * q + 0.15 * rand();
    const facing = out.clone().multiplyScalar(Math.cos(lift)).setZ(Math.sin(lift));
    const along = dead ? out.clone().multiplyScalar(0.45).setZ(-0.9) : dir;
    const r = dead ? 0.3 : 0.36 + 0.06 * rand();
    const tone = 0.9 + 0.2 * rand();
    const [fold, body, lit] = dead ? [straw.clone().multiplyScalar(0.75), straw, strawLit] : [deep, mid, tip].map((c) => c.clone().multiplyScalar(tone));
    const leaf = fan(hinge, facing, along, r, 11, dead ? 2.4 : 3.5 + 0.4 * rand(), rand);
    parts.push(paint(leaf, (p) => {
      const d = p.distanceTo(hinge) / r;
      return d > 0.7 ? lit.clone().lerp(body, 0.4 * hash(p, k)) : d > 0.3 ? fold : body.clone().lerp(fold, 0.5);
    }));
    parts.push(paint(ribbon([foot.toArray(), hinge.toArray()], [-out.y, out.x, 0], () => 0.03, 1), dead ? straw : stalk));
  }
  // Young leaves stand folded in the middle, so the clump has no hole at its heart.
  for (let k = 0; k < 3; k++) {
    const a = (k / 3) * 2 * Math.PI + rand();
    const length = 0.6 + 0.15 * rand();
    const spear = blade([0.015 * Math.cos(a), 0.015 * Math.sin(a), 0], a, length, 0.08 + 0.12 * rand(), 0.03, (t) => 0.07 * (1 - t) ** 0.7, 2);
    parts.push(paint(spear, (p) => mid.clone().lerp(tip, 0.3 + 0.5 * smooth(0.2, length, p.z))));
  }
  return merge(parts);
}

/**
 * A five-ribbed stem through `path` with radius `r` at each point, closed by a blunt cap rising
 * `cap` radii above its last point; a radius of 0 starts it in a point. Flat shaded, so the ribs
 * catch the light. `shade` gets the level along it: 0 at the foot and 1 at the top ring, then 1.5
 * at the cap's own corners on that ring and 2 at its tip. 10 triangles a segment, 5 for the cap
 * or a pointed foot.
 */
function stem(path: THREE.Vector3[], r: number[], cap: number, spin: number, shade: (p: THREE.Vector3, level: number) => THREE.Color) {
  const last = path.length - 1;
  const dirs = path.slice(1).map((p, i) => p.clone().sub(path[i]).normalize());
  // Each ring's frame comes down from the one above it, so a bent stem does not twist.
  let u = v3(Math.cos(spin), Math.sin(spin), 0);
  const rings: THREE.Vector3[][] = [];
  for (let i = last; i >= 0; i--) {
    const [a, b] = [dirs[Math.max(i - 1, 0)], dirs[Math.min(i, last - 1)]];
    const t = a.clone().add(b).normalize();
    u = u.projectOnPlane(t).normalize();
    const w = v3().crossVectors(t, u);
    // A ring at a bend lies in the mitre plane, stretched across the bend to keep the stem's girth.
    const bend = b.clone().sub(a).projectOnPlane(t);
    const stretch = 1 / t.dot(b) - 1;
    if (bend.lengthSq() > 1e-6) bend.normalize();
    rings[i] = [0, 1, 2, 3, 4].map((j) => {
      const off = u.clone().multiplyScalar(r[i] * Math.cos((j * 2 * Math.PI) / 5)).addScaledVector(w, r[i] * Math.sin((j * 2 * Math.PI) / 5));
      return path[i].clone().add(off).addScaledVector(bend, stretch * off.dot(bend));
    });
  }
  const apex = path[last].clone().addScaledVector(dirs[last - 1], cap * r[last]);
  const corners: THREE.Vector3[] = [];
  const levels: number[] = [];
  const push = (ps: THREE.Vector3[], ls: number[]) => ps.forEach((p, i) => (corners.push(p), levels.push(ls[i])));
  for (let i = 0; i < last; i++) {
    const [a, b] = [i / last, (i + 1) / last];
    for (let j = 0; j < 5; j++) {
      const n = (j + 1) % 5;
      if (r[i] > 0) push([rings[i][j], rings[i][n], rings[i + 1][n]], [a, a, b]);
      push([rings[i][j], rings[i + 1][n], rings[i + 1][j]], [a, b, b]);
    }
  }
  for (let j = 0; j < 5; j++) push([rings[last][j], rings[last][(j + 1) % 5], apex], [1.5, 1.5, 2]);
  const geo = new THREE.BufferGeometry()
    .setAttribute('position', new THREE.Float32BufferAttribute(corners.flatMap((p) => p.toArray()), 3))
    .setAttribute('color', new THREE.Float32BufferAttribute(corners.flatMap((p, i) => shade(p, levels[i]).toArray()), 3));
  geo.computeVertexNormals();
  return geo;
}

/**
 * Cardón: a dense candelabra mound of five-ribbed stems. Seven stand straight up in the middle;
 * ten round them leave the base sideways, narrow and low, and bend upright at an elbow. In flower
 * the tips and the tops of the stems below them turn red. 305 triangles.
 */
function cardon(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(CARDON[season]);
  const [flower, share] = CARDON_TIPS[season];
  const [wood, bloom] = [hex('#6a6450'), hex(flower)];
  const parts: THREE.BufferGeometry[] = [];
  const add = (path: THREE.Vector3[], r: number[]) => {
    const h = path[path.length - 1].z;
    const tone = 0.88 + 0.24 * rand();
    const flowering = rand() < share;
    const shade = (p: THREE.Vector3, level: number) => {
      const c = deep.clone().lerp(mid, smooth(0, 0.35, p.z)).lerp(tip, 0.55 * smooth(h - 0.3, h, p.z)).multiplyScalar(tone);
      c.lerp(wood, 0.6 * (1 - smooth(0, 0.12, p.z)));
      return flowering ? c.lerp(bloom, level > 1 ? 0.75 : 0.15 * smooth(0.99, 1, level)) : c;
    };
    parts.push(stem(path, r, 0.45, rand() * Math.PI, shade));
  };
  // A tight bundle in the middle, its stems touching: one at the centre and six around it.
  for (let k = 0; k < 7; k++) {
    const a = (k / 6) * 2 * Math.PI + rand() * 0.3;
    const out = v3(Math.cos(a), Math.sin(a), 0);
    const d = k === 6 ? 0.02 : 0.17 + 0.02 * rand();
    const r = 0.085 * (0.92 + 0.16 * rand());
    add([out.clone().multiplyScalar(0.7 * d).setZ(-0.03), out.clone().multiplyScalar(d + 0.03).setZ(1 - 0.5 * d - 0.1 * rand())], [0.85 * r, r]);
  }
  for (let k = 0; k < 10; k++) {
    const a = ((k + 0.5) / 10) * 2 * Math.PI + (rand() - 0.5) * 0.3;
    const out = v3(Math.cos(a), Math.sin(a), 0);
    const d = 0.44 + 0.08 * rand();
    const r = 0.085 * (0.92 + 0.16 * rand());
    const path = [out.clone().multiplyScalar(0.12).setZ(0), out.clone().multiplyScalar(d).setZ(0.12 + 0.05 * rand()), out.clone().multiplyScalar(d + 0.03).setZ(0.48 + 0.22 * rand())];
    add(path, [0, r, r]);
  }
  return merge(parts);
}

/**
 * Sweet spurge: a compact dome of foliage, its surface broken into rosettes of narrow fleshy
 * leaves, on grey branches that show beneath it. Each rosette is a star of leaves biased to its
 * top, lit as a small lump of the dome. The summer drought sheds leaves, so the dome shrinks and
 * fewer rosettes sit on it among bare twigs. 302 triangles, 242 in summer.
 */
function tabaiba(season: Season, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(TABAIBA[season]);
  const [bark, barkLit] = [hex('#57554e'), hex('#9a978a')];
  const dry = season === 'summer';
  const k = dry ? 0.85 : 1;
  const size = v3(0.5 * k, 0.5 * k, 0.56 * k);
  const position: number[] = [];
  const normal: number[] = [];
  const colour: number[] = [];
  const parts: THREE.BufferGeometry[] = [];
  const add = (corners: THREE.Vector3[], colours: THREE.Color[], centre: THREE.Vector3, r: number, around: THREE.Vector3) =>
    corners.forEach((p, i) => {
      const n = p.clone().sub(centre).divideScalar(r).addScaledVector(around, 0.8).normalize().lerp(UP, 0.2).normalize();
      position.push(p.x, p.y, p.z);
      normal.push(n.x, n.y, n.z);
      colours[i].toArray(colour, colour.length);
    });
  // The shade among the branches inside the bush, grey between the rosettes.
  const shell = soft(lathe([[0.95, 0.12], [1, 0.4], [0.75, 0.8], [0, 1]], 10, 3));
  shell.scale(0.85 * size.x, 0.85 * size.y, 0.85 * size.z);
  const inside = deep.clone().lerp(mid, 0.3).lerp(bark, 0.25);
  parts.push(paint(domed(shell, v3(0, 0, 0.1), size, 0.15), (p) => inside.clone().lerp(mid, 0.35 * smooth(0.15, 0.5, p.z))));
  for (let i = 0; i < 12; i++) {
    const a = i * GOLDEN + rand() * 0.4;
    const polar = Math.acos(1 - 0.85 * ((i + 0.5) / 12));
    const around = v3(Math.sin(polar) * Math.cos(a), Math.sin(polar) * Math.sin(a), Math.cos(polar));
    const centre = around.clone().multiply(size).setZ(0.08 + around.z * (size.z - 0.08)).addScaledVector(around, 0.02);
    // The low ones stand on branches; in summer every third has shed its leaves and is a bare twig.
    const bare = dry && i % 3 === 1;
    if (polar > 1 || bare) {
      const foot: Vec = [0.06 * Math.cos(a), 0.06 * Math.sin(a), -0.02];
      const end = bare ? centre.clone().addScaledVector(around, 0.04).addScaledVector(UP, 0.08) : centre.clone().addScaledVector(around, -0.1);
      parts.push(paint(soft(tube([foot, end.toArray()], 0.035, 0.5, 1, 3)), (p) => bark.clone().lerp(barkLit, smooth(0, 0.3, p.z))));
    }
    if (bare) continue;
    const axis = around.clone().lerp(UP, 0.3).normalize();
    const e1 = v3().crossVectors(axis, Math.abs(axis.z) > 0.9 ? v3(1, 0, 0) : UP).normalize();
    const e2 = v3().crossVectors(axis, e1);
    const r = (dry ? 0.19 : 0.26) * (0.9 + 0.2 * rand());
    const base = centre.clone().addScaledVector(axis, -0.3 * r);
    const sun = rand() < 0.4 ? 0.45 : 0.15 * rand();
    const low = 0.4 * (1 - smooth(0.15, 0.55, centre.z));
    for (let j = 0; j < 9; j++) {
      const t = a + j * GOLDEN + rand() * 0.5;
      // Denser towards the rosette's top and none past 75 degrees, so from above it shows no hole.
      const tilt = 1.3 * ((j + 0.5) / 9) ** 0.8;
      const across = e1.clone().multiplyScalar(Math.cos(t)).addScaledVector(e2, Math.sin(t));
      const dir = axis.clone().multiplyScalar(Math.cos(tilt)).addScaledVector(across, Math.sin(tilt));
      const length = r * (0.85 + 0.25 * rand());
      const end = base.clone().addScaledVector(dir, length);
      end.z -= 0.15 * length * Math.sin(tilt);
      const middle = base.clone().lerp(end, 0.55);
      const side = e1.clone().multiplyScalar(-Math.sin(t)).addScaledVector(e2, Math.cos(t)).multiplyScalar(0.03 * (0.8 + 0.4 * rand()));
      const leaf = mid.clone().lerp(tip, sun + 0.25 * rand()).lerp(deep, low + 0.2 * smooth(0.7, 1.3, tilt));
      const lit = leaf.clone().lerp(tip, 0.5);
      add([base, middle.clone().add(side), end, base, end, middle.clone().sub(side)], [deep, leaf, lit, deep, lit, leaf], centre, r, around);
    }
  }
  parts.push(
    new THREE.BufferGeometry()
      .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
      .setAttribute('normal', new THREE.Float32BufferAttribute(normal, 3))
      .setAttribute('color', new THREE.Float32BufferAttribute(colour, 3)),
  );
  return merge(parts.map((g) => (g.index ? g.toNonIndexed() : g)));
}

/** A leaf from `base` towards angle `a`, leaving at `tilt` from vertical, its tip dropping `arch` lengths. */
function blade(base: Vec, a: number, length: number, tilt: number, arch: number, width: (t: number) => number, segments: number) {
  const out: Vec = [Math.cos(a), Math.sin(a), 0];
  const along = (s: number): Vec => [
    base[0] + out[0] * Math.sin(tilt) * s * length,
    base[1] + out[1] * Math.sin(tilt) * s * length,
    base[2] + (Math.cos(tilt) * s - arch * s * s) * length,
  ];
  return ribbon([base, along(0.5), along(1)], [-out[1], out[0], 0], width, segments);
}

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

/**
 * Teide broom: an almost hemispherical bush of rush-thin upright twigs. A grey-olive shell carries
 * the mass; each twig is one sliver lying up it and standing out at its tip, so the twigs make
 * the outline. In flower the twig tips turn white, a few pink, and the shell pales a little. All
 * of it lights as the dome does. 270 triangles.
 */
function retama(season: Season, seed: number, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(RETAMA[season]);
  const [flower, share] = RETAMA_BLOOM[season];
  const bloom = season === 'spring';
  const [petal, pink] = [hex(flower), hex('#f0c8d0')];
  const size = v3(0.95, 0.95, 1);
  const radius = (n: THREE.Vector3) => 1 + 0.05 * noise(n, 2.5, seed);
  // Flowers deep in the bush pale its shade in bloom, still grey-olive.
  const inside = deep.clone().lerp(mid, 0.25).lerp(hex('#c8c4a8'), bloom ? 0.3 : 0);
  const position: number[] = [];
  const normal: number[] = [];
  const colour: number[] = [];
  const count = 210;
  for (let j = 0; j < count; j++) {
    const polar = Math.acos(1 - (j + 0.5) / count) * (0.97 + 0.04 * rand());
    const a = j * GOLDEN + rand() * 0.3;
    const n = v3(Math.sin(polar) * Math.cos(a), Math.sin(polar) * Math.sin(a), Math.cos(polar));
    const surface = n.clone().multiply(size).multiplyScalar(0.92 * radius(n));
    // Up the dome along its meridian but steeper, as broom twigs stand upright, so each lies on
    // the dome as a long streak and its tip stands out of it.
    const meridian = v3(-Math.cos(polar) * Math.cos(a), -Math.cos(polar) * Math.sin(a), Math.sin(polar));
    const dir = meridian.addScaledVector(UP, 0.8).addScaledVector(n, 0.25).normalize();
    const length = 0.32 + 0.12 * rand();
    const top = surface.clone().addScaledVector(dir, 0.45 * length);
    const foot = surface.clone().addScaledVector(dir, -0.55 * length).addScaledVector(n, -0.04);
    foot.z = Math.max(foot.z, 0);
    const twig = mid.clone().lerp(tip, 0.1 + 0.5 * rand());
    const flowers = hash(top, seed) < share;
    const end = flowers ? twig.clone().lerp(bloom && rand() < 0.18 ? pink : petal, bloom ? 0.95 : 0.5) : twig;
    // Flowering twigs are a little wider, as their flowers stand off them.
    const across = v3().crossVectors(dir, n);
    if (across.lengthSq() < 0.01) across.set(-Math.sin(a), Math.cos(a), 0);
    across.setLength((bloom && flowers ? 0.02 : 0.012) + 0.004 * rand());
    const lit = n.clone().lerp(UP, 0.15).normalize();
    const corners = [foot.clone().add(across), foot.clone().sub(across), top];
    const colours = [inside, twig.clone().lerp(inside, 0.5), end];
    corners.forEach((c, i) => {
      position.push(c.x, c.y, c.z);
      normal.push(lit.x, lit.y, lit.z);
      colours[i].toArray(colour, colour.length);
    });
  }
  const sprays = new THREE.BufferGeometry()
    .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
    .setAttribute('normal', new THREE.Float32BufferAttribute(normal, 3))
    .setAttribute('color', new THREE.Float32BufferAttribute(colour, 3));
  const core = soft(lathe([[0.92, -0.03], [0.88, 0.42], [0.55, 0.8], [0, 0.9]], 12, 3));
  core.scale(0.9 * size.x, 0.9 * size.y, 0.9 * size.z);
  const shell = paint(domed(core, v3(), size, 0.1), (p) => inside.clone().lerp(mid, 0.3 * smooth(0.2, 0.8, p.z)));
  return merge([shell.toNonIndexed(), sprays]);
}

export const SUCCULENTS = {
  'fan-palm': { name: 'Dwarf fan palm', build: (season: Season, seed: number) => fanPalm(season, random(seed)) },
  cardon: { name: 'Cardón', build: (season: Season, seed: number) => cardon(season, random(seed)) },
  tabaiba: { name: 'Tabaiba', build: (season: Season, seed: number) => tabaiba(season, random(seed)) },
  tajinaste: { name: 'Tajinaste', build: (season: Season, seed: number) => tajinaste(season, seed, random(seed)) },
  retama: { name: 'Retama', build: (season: Season, seed: number) => retama(season, seed, random(seed)) },
};
