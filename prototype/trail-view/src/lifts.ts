/**
 * Cable cars and ski lifts from OSM's aerialway lines: a station at each end, a pylon at each
 * vertex between, two cables sagging from pylon to pylon, and carriers spaced along them. OSM
 * often maps a lift station to station only, so pylons are added where a span runs longer than the
 * kind allows or its cable would graze the ground: on the knolls, where real pylons stand.
 */

import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { groundAt, type Bake } from './bake';
import { cabin, CABLE_AT_STATION_M, pylon, station, TRACK_HALF_M, type AerialwayKind } from './models/aerialways';

const KINDS: Record<string, AerialwayKind> = {
  cable_car: 'cable_car', gondola: 'gondola', mixed_lift: 'gondola', chair_lift: 'chair_lift',
  drag_lift: 'drag_lift', 't-bar': 'drag_lift', 'j-bar': 'drag_lift', platter: 'drag_lift',
};

/**
 * Per kind: the pylon's height, the longest span between pylons, the cable's sag as a share of the
 * span, how far a carrier hangs below the cable and the clearance it keeps over the ground, and the
 * spacing of carriers (0 for a cable car's two cabins).
 */
const RIG: Record<AerialwayKind, { pylon: number; span: number; sag: number; drop: number; clear: number; spacing: number }> = {
  cable_car: { pylon: 30, span: Infinity, sag: 0.04, drop: 5.3, clear: 10, spacing: 0 },
  gondola: { pylon: 14, span: 300, sag: 0.02, drop: 3.7, clear: 4, spacing: 70 },
  chair_lift: { pylon: 10, span: 180, sag: 0.02, drop: 4, clear: 2.5, spacing: 30 },
  drag_lift: { pylon: 7, span: 120, sag: 0.01, drop: 2.9, clear: 1, spacing: 15 },
};
/** Gondolas, chairs and T-bars are a pixel or two beyond this; cable car cabins are drawn at any range. */
const CARRIER_REACH_M = 1500;
const CABLE = new THREE.Color('#2b2b2b');
/** Samples per span when testing the cable's clearance. */
const SAMPLES = 16;
/** An added pylon stands at least this far from the anchors beside it. */
const PYLON_GAP_M = 25;

interface Anchor {
  x: number;
  y: number;
  /** The cable's height here, above the eye. */
  z: number;
}

/** The cable's height t of the way from a to b: straight, less a parabolic sag. */
const cableZ = (a: Anchor, b: Anchor, t: number, sag: number) =>
  a.z + (b.z - a.z) * t - 4 * sag * Math.hypot(b.x - a.x, b.y - a.y) * t * (1 - t);

/** The anchors of one lift: its stations and pylons, with pylons added where the cable needs them. */
function anchors(bake: Bake, kind: AerialwayKind, line: number[]): Anchor[] {
  const rig = RIG[kind];
  const at = (x: number, y: number, height: number): Anchor => ({ x, y, z: groundAt(bake, x, y) + height });
  const out: Anchor[] = [];
  const last = line.length / 2 - 1;
  for (let i = 0; i <= last; i++) out.push(at(line[2 * i], line[2 * i + 1], i === 0 || i === last ? CABLE_AT_STATION_M[kind] : rig.pylon));
  // Split spans until each is short enough and clears the ground.
  for (let i = 0; i < out.length - 1 && out.length < 200; ) {
    const [a, b] = [out[i], out[i + 1]];
    const span = Math.hypot(b.x - a.x, b.y - a.y);
    let worst = { t: 0, by: 0 };
    for (let s = 1; s < SAMPLES; s++) {
      const t = s / SAMPLES;
      const below = groundAt(bake, a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t) + rig.drop + rig.clear - cableZ(a, b, t, rig.sag);
      if (below > worst.by) worst = { t, by: below };
    }
    const t = span > rig.span ? 0.5 : worst.t;
    if ((span > rig.span || worst.by > 0) && span * Math.min(t, 1 - t) > PYLON_GAP_M) {
      out.splice(i + 1, 0, at(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, rig.pylon));
    } else i++;
  }
  return out;
}

/** Each lift's stations, pylons and carriers as one mesh, and its cables as lines. */
export function lifts(bake: Bake, reach: number): { solid: THREE.BufferGeometry | null; cables: THREE.BufferGeometry | null } {
  const parts: THREE.BufferGeometry[] = [];
  const cable: number[] = [];
  const models = new Map<string, THREE.BufferGeometry>();
  // Merging needs the same attributes everywhere, unindexed.
  const model = (key: string, build: () => THREE.BufferGeometry) => {
    if (!models.has(key)) {
      const built = build();
      const flat = built.index ? built.toNonIndexed() : built;
      models.set(key, new THREE.BufferGeometry()
        .setAttribute('position', flat.getAttribute('position'))
        .setAttribute('normal', flat.getAttribute('normal'))
        .setAttribute('color', flat.getAttribute('color')));
    }
    return models.get(key)!;
  };
  bake.aerialways.forEach(({ kind: tag, line }, n) => {
    const kind = KINDS[tag];
    const nearest = Math.min(...line.filter((_, i) => i % 2 === 0).map((x, i) => Math.hypot(x, line[2 * i + 1])));
    if (!kind || line.length < 4 || nearest > reach) return;
    const rig = RIG[kind];
    const points = anchors(bake, kind, line);
    // The heading of the cable at each anchor: along the line, bisecting at a bend.
    const heading = points.map((_, i) => {
      const [a, b] = [points[Math.max(i - 1, 0)], points[Math.min(i + 1, points.length - 1)]];
      return Math.atan2(b.x - a.x, b.y - a.y);
    });
    points.forEach((p, i) => {
      const end = i === 0 || i === points.length - 1;
      // Models carry the cable along +y; the far station looks back down the line.
      const turn = -heading[i] + (i === points.length - 1 ? Math.PI : 0);
      // One seed per line, so its stations, pylons and cable car cabins share a livery.
      const geometry = end ? model(`station|${kind}|${n}`, () => station(kind, n)) : model(`pylon|${kind}|${n}`, () => pylon(kind, rig.pylon, n));
      parts.push(geometry.clone().rotateZ(turn).translate(p.x, p.y, p.z - (end ? CABLE_AT_STATION_M[kind] : rig.pylon)));
    });
    // Two tracks, each offset across its span; carriers ride them a half spacing apart.
    const length = points.slice(1).reduce((sum, p, i) => sum + Math.hypot(p.x - points[i].x, p.y - points[i].y), 0);
    const stops = rig.spacing ? Math.max(1, Math.floor(length / rig.spacing)) : 1;
    for (const side of [-1, 1]) {
      const along = rig.spacing ? [...Array(stops).keys()].map((s) => (s + (side > 0 ? 0.5 : 0)) * (length / stops)) : [length * (side > 0 ? 0.35 : 0.65)];
      let start = 0;
      for (let i = 0; i < points.length - 1; i++) {
        const [a, b] = [points[i], points[i + 1]];
        const span = Math.hypot(b.x - a.x, b.y - a.y);
        const [ox, oy] = [(Math.cos(heading[i]) * TRACK_HALF_M[kind] * side), (-Math.sin(heading[i]) * TRACK_HALF_M[kind] * side)];
        const [ex, ey] = [(Math.cos(heading[i + 1]) * TRACK_HALF_M[kind] * side), (-Math.sin(heading[i + 1]) * TRACK_HALF_M[kind] * side)];
        const point = (t: number) => [a.x + ox + (b.x + ex - a.x - ox) * t, a.y + oy + (b.y + ey - a.y - oy) * t, cableZ(a, b, t, rig.sag)];
        const steps = Math.max(2, Math.ceil(span / 20));
        for (let s = 0; s < steps; s++) cable.push(...point(s / steps), ...point((s + 1) / steps));
        for (const d of along.filter((d) => d >= start && d < start + span)) {
          const [x, y, z] = point((d - start) / span);
          if (kind !== 'cable_car' && Math.hypot(x, y) > CARRIER_REACH_M) continue;
          // A gondola's cabins come in a few liveries. A hanger bends out to +x past the sheaves, so
          // carriers on the -x track turn about.
          const seed = kind === 'gondola' ? Math.floor(d / rig.spacing) % 5 : n;
          const turn = -Math.atan2(b.x - a.x, b.y - a.y) + (side < 0 ? Math.PI : 0);
          parts.push(model(`cabin|${kind}|${seed}`, () => cabin(kind, seed)).clone().rotateZ(turn).translate(x, y, z));
        }
        start += span;
      }
    }
  });
  let cables: THREE.BufferGeometry | null = null;
  if (cable.length) {
    const count = cable.length / 3;
    cables = new THREE.BufferGeometry()
      .setAttribute('position', new THREE.Float32BufferAttribute(cable, 3))
      .setAttribute('normal', new THREE.Float32BufferAttribute(new Float32Array(count * 3).map((_, i) => (i % 3 === 2 ? 1 : 0)), 3))
      .setAttribute('color', new THREE.Float32BufferAttribute(new Float32Array(count * 3).map((_, i) => CABLE.toArray()[i % 3]), 3));
  }
  return { solid: parts.length ? mergeGeometries(parts) : null, cables };
}
