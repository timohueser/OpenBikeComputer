import { KINDS, POINT_MODELS } from '../kinds';
import { placed, type Build, type Placement } from '../kinds/kind';
import { packPoints, type Point } from '../instances/points';
import type { Feature } from '../stream/features';
import { AnchoredBuilder, anchorBytes, type AnchoredMesh } from './anchored';
import { BRIDGE, features, metresPerUnit, PATH, ROAD, TUNNEL, type Node } from '../terrain/bins';
import { cornerAt } from '../terrain/corners';
import { bridges, deckHalf } from './bridges';
import { galleries } from './galleries';
import { MeshBuilder, type StructureMesh } from './mesh';
import { rails } from './rails';
import { sidewalks } from './sidewalks';
import { boreHalf, portalBox, tunnels, twins } from './tunnels';

/**
 * The level of the nodes that hold the structures: each piece belongs to the one whose square holds its
 * anchor (a segment's first vertex, a pier, a portal), so every piece draws once and no node clips.
 * Coarser nodes hold none: farther off a deck spans a few pixels.
 */
export const OWNER = 15;
/** A portal's box reaches this far from it. */
const REACH_M = 20;

/** Whether a world point lies in a square, half-open. */
export type Owns = (x: number, y: number) => boolean;

/** Words of a floor segment: its ends in world units, their heights, and its half-width in metres: out to a deck's parapets, to a bore's walls. */
export const FLOOR = 7;
/** A deck's road lies this far over its line's height, as LIFT_M of bridges.ts. */
const DECK_LIFT_M = 0.03;

/**
 * The floors that stand in an OWNER node's square: each segment of a deck's road and of a tunnel's road
 * that comes within REACH_M of it, as FLOOR words. Null without floors.
 */
export function floorsOf(list: Feature[], tile: Node, node: Node): Float64Array<ArrayBuffer> | null {
    if (node.level !== OWNER) return null;
    const side = 2 ** (30 - node.level), x0 = node.x * side, y0 = node.y * side, m = REACH_M / metresPerUnit(y0 + side / 2), out: number[] = [];
    for (const line of features(list, tile).lines) {
        if (line.kind !== BRIDGE && line.kind !== TUNNEL) continue;
        const lift = line.kind === BRIDGE ? DECK_LIFT_M : 0;
        for (let v = 0; v + 1 < line.z.length; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4);
            if (Math.max(ax, bx) < x0 - m || Math.min(ax, bx) > x0 + side + m || Math.max(ay, by) < y0 - m || Math.min(ay, by) > y0 + side + m) continue;
            out.push(ax, ay, bx, by, line.z[v] + lift, line.z[v + 1] + lift, line.kind === BRIDGE ? deckHalf(line) : boreHalf(line));
        }
    }
    return out.length ? new Float64Array(out) : null;
}

/** Words of a portal's box: its facade's middle in world units, its unit way into the tunnel, and its `portalBox`. */
export const BOX = 9;

/** The boxes of the tunnels' portals within REACH_M of a node's square, where the terrain draws nothing; null without any. */
export function portalBoxes(list: Feature[], tile: Node, node: Node): Float64Array<ArrayBuffer> | null {
    const side = 2 ** (30 - node.level), x0 = node.x * side, y0 = node.y * side, m = REACH_M / metresPerUnit(y0 + side / 2), out: number[] = [];
    const { lines } = features(list, tile), twin = twins(lines);
    for (const line of lines) {
        if (line.kind !== TUNNEL || !line.abut) continue;
        for (const end of [0, 1]) {
            const v = end ? line.z.length - 1 : 0, w = end ? v - 1 : 1, x = line.xy[2 * v], y = line.xy[2 * v + 1], pair = twin.get(line)?.[end];
            if (!line.abut[end] || pair === null || x < x0 - m || x >= x0 + side + m || y < y0 - m || y >= y0 + side + m) continue;
            // A portal over two tracks: its middle moves to the left of the way in, as `portal` of tunnels.ts moves it.
            const dx = line.xy[2 * w] - x, dy = line.xy[2 * w + 1] - y, l = Math.hypot(dx, dy) || 1, shift = (pair?.shift ?? 0) / metresPerUnit(y);
            out.push(x + dy / l * shift, y - dx / l * shift, dx / l, dy / l, ...portalBox(line, v, end ? 0 : line.z.length - 1, pair?.widen));
        }
    }
    return out.length ? new Float64Array(out) : null;
}

/** Meshes of one reach: the structures, and the anchored mesh. */
export interface Meshes { structures: StructureMesh | null; anchored: AnchoredMesh | null }
/**
 * A node's meshes: those that draw at any distance, those that draw only near the eye, and the anchors of
 * both; and the models on the surface of its kinds, as `packPoints` packs them, the model the number of
 * POINT_MODELS.
 */
export interface NodeMeshes { far: Meshes; near: Meshes; anchors: ArrayBuffer | null; points: Uint32Array<ArrayBuffer> | null }

/** The meshes of a node of the OWNER level from the features of its feature tile: the structures, then the kinds of the registry. `corridors` hold the sidewalks themselves. */
export function structureMeshes(list: Feature[], tile: Node, node: Node, corridors = false): NodeMeshes {
    const none = { structures: null, anchored: null };
    if (node.level !== OWNER) return { far: none, near: none, anchors: null, points: null };
    const side = 2 ** (30 - node.level), x0 = node.x * side, y0 = node.y * side;
    const owns: Owns = (x, y) => x >= x0 && x < x0 + side && y >= y0 && y < y0 + side;
    const words: number[] = [], far = { mesh: new MeshBuilder(), anchored: new AnchoredBuilder(words) }, near = { mesh: new MeshBuilder(), anchored: new AnchoredBuilder(words) };
    const { lines, buildings } = features(list, tile);
    if (!corridors) sidewalks(near.mesh, lines, [x0, y0, x0 + side, y0 + side]);
    bridges(far.mesh, list, tile, lines, owns);
    tunnels(far.mesh, lines, owns);
    galleries(far.mesh, lines, owns);
    rails(far.mesh, far.anchored, list, tile, lines, owns, [x0, y0, x0 + side, y0 + side]);
    const points: Point[] = [];
    let kind = '';
    const place = ({ model, tint = 1, ...at }: Placement) => points.push({ ...at, tint, model: POINT_MODELS.indexOf(`${kind}/${model}`) });
    const out: Build = { far, near, owns, place, ways: lines.filter(line => line.kind === ROAD || line.kind === PATH), buildings, cornered: (x, y) => !!cornerAt(lines, x, y) };
    for (const feature of list) {
        kind = feature.kind;
        KINDS.get(kind)?.build(placed(feature, tile), out);
    }
    const meshes = ({ mesh, anchored }: typeof far): Meshes => ({ structures: mesh.mesh(), anchored: anchored.mesh() });
    return { far: meshes(far), near: meshes(near), anchors: words.length ? anchorBytes(words) : null, points: points.length ? packPoints(points.filter(p => p.model >= 0)) : null };
}
