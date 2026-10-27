// What a kind module is, and the shared handlers that most kinds use. A kind module in kinds/ has the name
// of its entry in data/trailview/kinds.json and default-exports a Kind; the registry (index.ts) finds it,
// so a new kind changes no other file of the engine.
import { geometry, type Model } from '../models/flat';
import { EXTENT, unitScale, type Feature } from '../stream/features';
import type { ModelSpec } from '../instances/instances';
import type { AnchoredBuilder } from '../structures/anchored';
import { GROUND, mix, type MeshBuilder, station, type Corner } from '../structures/mesh';
import type { Owns } from '../structures/node';
import { metresPerUnit, type Line, type Node } from '../terrain/bins';

/**
 * A feature of a tile in world units: each part's x, y in turn. The cut of a tile keeps one vertex past
 * the tile's buffered square, so a part's first or last vertex that a node of the tile owns ends the line.
 */
export interface Placed {
    tags: Record<string, string>;
    parts: { outer: boolean; xy: number[] }[];
}

/** A model on the surface for the instances: its foot at world point x, y, its +x turned `yaw` radians counter-clockwise from east, its height in metres, a model of the kind by name and a tint, 0.75 to 1.25. */
export interface Placement { x: number; y: number; yaw: number; height: number; model: string; tint?: number }

/**
 * What a kind builds for a node, in the stamp worker, only of the pieces that the node `owns`: faces on
 * the drawn surface (`mesh`) and anchored models and cables (`anchored`), in `far`, which draws at any
 * distance, or in `near`, which draws only near the eye; and models on the surface (`place`), which the
 * instances stand on the drawn surface, cull and draw in tiers.
 */
export interface Build {
    far: { mesh: MeshBuilder; anchored: AnchoredBuilder };
    near: { mesh: MeshBuilder; anchored: AnchoredBuilder };
    place: (placement: Placement) => void;
    owns: Owns;
}

export interface Kind {
    build(feature: Placed, out: Build): void;
    /** The models that `place` names, each 1 tall with its base at the origin, for the instances. */
    models?: Record<string, () => ModelSpec>;
}

/** A feature of a tile in world units. */
export function placed({ tags, parts }: Feature, tile: Node): Placed {
    const scale = unitScale(tile.level), ox = tile.x * EXTENT, oy = tile.y * EXTENT;
    return { tags, parts: parts.map(({ outer, xy }) => ({ outer, xy: Array.from(xy, (v, i) => ((i % 2 ? oy : ox) + v) * scale) })) };
}

/** A model on the surface at each point of a feature that the node owns. */
export function pointModels(feature: Placed, out: Build, instance: (x: number, y: number) => Omit<Placement, 'x' | 'y'> | undefined): void {
    for (const { xy } of feature.parts) for (let i = 0; i < xy.length; i += 2) {
        if (!out.owns(xy[i], xy[i + 1])) continue;
        const it = instance(xy[i], xy[i + 1]);
        if (it) out.place({ x: xy[i], y: xy[i + 1], ...it });
    }
}

/**
 * A kind of one model at each point: `model` in metres with its base at the origin, turned by the hash of
 * its place, as the map does not tell which way it faces. It draws to `reach` metres.
 */
export function modelKind(name: string, model: () => Model, reach: number): Kind {
    const height = Math.max(...model().points.filter((_, i) => i % 3 === 2));
    return {
        build(feature, out) {
            pointModels(feature, out, (x, y) => ({ yaw: ((x * 7 + y * 13) % 628) / 100, height, model: name }));
        },
        models: { [name]: () => ({ tiers: [{ mesh: geometry(model().place(0, [0, 0, 0], 1 / height)), until: reach }] }) },
    };
}

/** The smallest rectangle about a ring, one side along an edge: its middle in world units, the angle of its width counter-clockwise from east, and its width and depth in metres. */
export function footprint(ring: number[]): { x: number; y: number; angle: number; width: number; depth: number } {
    const m = metresPerUnit(ring[1]), n = ring.length / 2, p = (i: number) => [(ring[2 * i] - ring[0]) * m, -(ring[2 * i + 1] - ring[1]) * m];
    let best = { area: Infinity, x: 0, y: 0, angle: 0, width: 0, depth: 0 };
    for (let i = 0; i < n; i++) {
        const [ax, ay] = p(i), [bx, by] = p((i + 1) % n), angle = Math.atan2(by - ay, bx - ax), c = Math.cos(angle), s = Math.sin(angle);
        let [u0, u1, v0, v1] = [Infinity, -Infinity, Infinity, -Infinity];
        for (let k = 0; k < n; k++) {
            const [x, y] = p(k), u = x * c + y * s, v = -x * s + y * c;
            [u0, u1, v0, v1] = [Math.min(u0, u), Math.max(u1, u), Math.min(v0, v), Math.max(v1, v)];
        }
        const area = (u1 - u0) * (v1 - v0);
        if (area >= best.area) continue;
        const u = (u0 + u1) / 2, v = (v0 + v1) / 2;
        best = { area, x: u * c - v * s, y: u * s + v * c, angle, width: u1 - u0, depth: v1 - v0 };
    }
    return { x: ring[0] + best.x / m, y: ring[1] - best.y / m, angle: best.angle, width: best.width, depth: best.depth };
}

/** A cross-section [across to the left, up] from its foot round over its top back to its foot, the colour of a line at a point, and the longest piece in metres. */
export interface Section { corners: Corner[]; colour: (x: number, y: number) => number; step: number }

/**
 * A cross-section drawn along each line of a feature on the drawn surface, closed at the line's ends.
 * A segment's pieces are at most `step` metres long, so it follows the ground; the node that owns a
 * segment's first vertex draws it, and the node that owns an end draws its cap.
 */
export function along(feature: Placed, mesh: MeshBuilder, owns: Owns, { corners, colour, step }: Section): void {
    for (const { xy } of feature.parts) {
        const n = xy.length / 2;
        if (n < 2) continue;
        const line: Line = { kind: 0, xy, z: new Array(n).fill(0), h: 0, code: 0, ford: false, profile: 0 };
        const paint = (v: number) => (colour(xy[2 * v], xy[2 * v + 1]) | (GROUND << 24)) >>> 0;
        for (let v = 0; v + 1 < n; v++) {
            if (!owns(xy[2 * v], xy[2 * v + 1])) continue;
            const s = station(line, v), t = station(line, v + 1), c = paint(v);
            const pieces = Math.max(1, Math.ceil(Math.hypot(xy[2 * v + 2] - xy[2 * v], xy[2 * v + 3] - xy[2 * v + 1]) * metresPerUnit(xy[2 * v + 1]) / step));
            for (let k = 0; k < pieces; k++) {
                const p = mix(s, t, k / pieces), q = mix(s, t, (k + 1) / pieces);
                corners.slice(1).forEach((b, i) => mesh.strip(p, q, corners[i], b, c, [b[1] - corners[i][1], corners[i][0] - b[0]]));
            }
        }
        // A closed line has no ends.
        const closed = xy[0] === xy[2 * n - 2] && xy[1] === xy[2 * n - 1];
        for (const [v, w] of [[0, 1], [n - 1, n - 2]]) {
            if (closed || !owns(xy[2 * v], xy[2 * v + 1])) continue;
            // The cap faces out of the line, in the colour of the segment that it closes.
            const at = station(line, v), ux = xy[2 * v] - xy[2 * w], uy = xy[2 * v + 1] - xy[2 * w + 1], l = Math.hypot(ux, uy) || 1;
            const ring = corners.map(corner => mesh.at(at, corner, paint(Math.min(v, w))));
            for (let i = 1; i + 1 < ring.length; i++) mesh.triangle(ring[0], ring[i], ring[i + 1], [ux / l, -uy / l, 0]);
        }
    }
}
