/**
 * Bridges from OSM ways tagged bridge, which the road field leaves unpainted: a deck from bank to
 * bank, level with the ground at each end, parapets along its edges, and under it arches of at
 * most ARCH_M springing from piers, with the spandrel walls between arch and deck solid.
 */

import * as THREE from 'three';
import { groundAt, type Bake } from './bake';
import { ROADS } from './palette';
import type { Region } from './regions';

/** The longest arch, the deck's depth, the parapet's height and thickness, and a pier's length. */
const ARCH_M = 25;
const DECK_M = 0.6;
const PARAPET_M = [1, 0.3];
const PIER_M = 1.5;
/** An arch rises this share of its span above its springs, at most as much as the room under it. */
const RISE = 0.3;
/** Deck samples along the line, about this far apart. */
const STEP_M = 2;

type Vec = number[];

/**
 * Every bridge in flat-shaded meshes in vertex colours, metres from the eye: the decks and the
 * stonework, each null where there is none. Each bridge takes the region at its middle: the deck its
 * road colour by surface code, as the painted road beside it does, and the stone its house style's
 * plinth.
 */
export function bridges(bake: Bake, regionAt: (x: number, y: number) => Region): (THREE.BufferGeometry | null)[] {
  const [decks, stonework] = [{ position: [] as number[], color: [] as number[] }, { position: [] as number[], color: [] as number[] }];
  // The object shader lights by the winding's normal, so each face is wound to face `out`.
  const quad = (a: Vec, b: Vec, c: Vec, d: Vec, colour: THREE.Color, out: Vec, into = stonework) => {
    const [u, v] = [[c[0] - a[0], c[1] - a[1], c[2] - a[2]], [d[0] - b[0], d[1] - b[1], d[2] - b[2]]];
    const facing = (u[1] * v[2] - u[2] * v[1]) * out[0] + (u[2] * v[0] - u[0] * v[2]) * out[1] + (u[0] * v[1] - u[1] * v[0]) * out[2];
    for (const p of facing >= 0 ? [a, b, c, a, c, d] : [a, c, b, a, d, c]) {
      into.position.push(...p);
      into.color.push(colour.r, colour.g, colour.b);
    }
  };
  const [up, down] = [[0, 0, 1], [0, 0, -1]];
  for (const { code, half, line } of bake.bridges) {
    const points: [number, number][] = [];
    for (let i = 0; i < line.length; i += 2) points.push([line[i], line[i + 1]]);
    const along = [0];
    for (let i = 1; i < points.length; i++) along.push(along[i - 1] + Math.hypot(points[i][0] - points[i - 1][0], points[i][1] - points[i - 1][1]));
    const length = along[along.length - 1];
    if (length < 2) continue;
    // A point, the unit normal to its left and the unit direction, s metres along the line.
    const at = (s: number) => {
      s = Math.min(Math.max(s, 0), length);
      let i = 0;
      while (i < points.length - 2 && along[i + 1] < s) i++;
      const t = (s - along[i]) / Math.max(along[i + 1] - along[i], 1e-6);
      const [[ax, ay], [bx, by]] = [points[i], points[i + 1]];
      const l = Math.max(Math.hypot(bx - ax, by - ay), 1e-6);
      return { x: ax + (bx - ax) * t, y: ay + (by - ay) * t, nx: -(by - ay) / l, ny: (bx - ax) / l };
    };
    type At = ReturnType<typeof at>;
    const middle = at(length / 2);
    const { look, pack } = regionAt(middle.x, middle.y);
    const roads = look.roads ?? ROADS;
    const surface = new THREE.Color(roads[code - 1] ?? roads[1]);
    const stone = new THREE.Color(pack.style.palette.plinth);
    const dark = stone.clone().multiplyScalar(0.75);
    const edge = (o: At, side: number, z: number, inset = 0) => [o.x + o.nx * side * (half - inset), o.y + o.ny * side * (half - inset), z];
    const ground = (o: At, side: number) => groundAt(bake, o.x + o.nx * side * half, o.y + o.ny * side * half);
    const outward = (o: At, side: number) => [o.nx * side, o.ny * side, 0];
    const [z0, z1] = [groundAt(bake, ...points[0]), groundAt(bake, ...points[points.length - 1])];
    const deck = (s: number) => z0 + ((z1 - z0) * s) / length;
    const low = (o: At) => Math.min(ground(o, 1), ground(o, -1));
    const arches = Math.max(1, Math.ceil(length / ARCH_M));
    const span = length / arches;
    // Samples every STEP_M or so, and at each spring and pier face, so walls meet the piers.
    const steps = Math.max(2, Math.ceil(length / STEP_M));
    const marks = [...Array(steps + 1).keys()].map((k) => (k * length) / steps);
    // A low crossing, a beam a little above the stream, gets a flat arch.
    const room = Math.max(...marks.map((s) => deck(s) - DECK_M - low(at(s))));
    const rise = Math.min(RISE * span, Math.max(room - 0.5, 0));
    // The arch's underside: highest at mid-span, down to its springs at the piers and banks.
    const soffit = (s: number) => {
      const u = Math.min(Math.max(s / span - Math.floor(s / span), 0), 1);
      return deck(s) - DECK_M - rise * (2 * u - 1) ** 2;
    };
    for (let a = 1; a < arches; a++) marks.push(a * span - PIER_M / 2, a * span, a * span + PIER_M / 2);
    marks.sort((a, b) => a - b);
    const [h, w] = PARAPET_M;
    for (let k = 0; k + 1 < marks.length; k++) {
      const [s0, s1] = [marks[k], marks[k + 1]];
      if (s1 - s0 < 1e-3) continue;
      const [p, q] = [at(s0), at(s1)];
      quad(edge(p, 1, deck(s0)), edge(p, -1, deck(s0)), edge(q, -1, deck(s1)), edge(q, 1, deck(s1)), surface, up, decks);
      for (const side of [1, -1]) {
        const out = outward(p, side);
        // The spandrel wall from the deck down to the arch or the ground, never above the deck's
        // underside, where an approach runs on the ground.
        const bottom = (o: At, s: number) => Math.min(deck(s) - DECK_M, Math.max(soffit(s), ground(o, side) - 0.5));
        quad(edge(p, side, deck(s0)), edge(q, side, deck(s1)), edge(q, side, bottom(q, s1)), edge(p, side, bottom(p, s0)), stone, out);
        quad(edge(p, side, deck(s0) + h, w), edge(q, side, deck(s1) + h, w), edge(q, side, deck(s1) + h), edge(p, side, deck(s0) + h), stone, up);
        quad(edge(p, side, deck(s0)), edge(q, side, deck(s1)), edge(q, side, deck(s1) + h), edge(p, side, deck(s0) + h), stone, out);
        quad(edge(p, side, deck(s0), w), edge(q, side, deck(s1), w), edge(q, side, deck(s1) + h, w), edge(p, side, deck(s0) + h, w), stone, [-out[0], -out[1], 0]);
      }
      // The arch's underside where it clears the ground at either edge.
      if (soffit(s0) > low(p) || soffit(s1) > low(q))
        quad(edge(p, -1, soffit(s0)), edge(p, 1, soffit(s0)), edge(q, 1, soffit(s1)), edge(q, -1, soffit(s1)), dark, down);
    }
    // Piers from the arches at their faces down to the ground.
    for (let a = 1; a < arches; a++) {
      const s = a * span;
      const [p, q] = [at(s - PIER_M / 2), at(s + PIER_M / 2)];
      const [tp, tq] = [soffit(s - PIER_M / 2), soffit(s + PIER_M / 2)];
      const foot = Math.min(ground(p, 1), ground(p, -1), ground(q, 1), ground(q, -1)) - 0.5;
      if (Math.max(tp, tq) <= foot) continue;
      for (const side of [1, -1])
        quad(edge(p, side, tp), edge(q, side, tq), edge(q, side, foot), edge(p, side, foot), stone, outward(p, side));
      quad(edge(p, 1, tp), edge(p, -1, tp), edge(p, -1, foot), edge(p, 1, foot), stone, [-p.ny, p.nx, 0]);
      quad(edge(q, 1, tq), edge(q, -1, tq), edge(q, -1, foot), edge(q, 1, foot), stone, [q.ny, -q.nx, 0]);
    }
  }
  return [decks, stonework].map(({ position, color }) => {
    if (!position.length) return null;
    const geometry = new THREE.BufferGeometry()
      .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
      .setAttribute('color', new THREE.Float32BufferAttribute(color, 3));
    geometry.computeVertexNormals();
    return geometry;
  });
}
