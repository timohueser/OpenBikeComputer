import { KINDS, POINT_MODELS } from '../kinds';
import { placed, type Build, type Placement } from '../kinds/kind';
import { coresOf } from '../roads/cores';
import { packPoints, type Point } from '../instances/points';
import type { Feature } from '../stream/features';
import { AnchoredBuilder, anchorBytes, type AnchoredMesh } from './anchored';
import { BRIDGE, features, insideRings, LIFT_M, metresPerUnit, TUNNEL, type Node } from '../terrain/bins';
import { OWNER } from '../terrain/lod';
import { cornerAt } from '../terrain/corners';
import { deckHalf } from '../roads/decks';
import { bridges } from './bridges';
import { galleries } from './galleries';
import { MeshBuilder, type StructureMesh } from './mesh';
import { rails } from './rails';
import { boreHalf, portalBox, tunnels, twins } from './tunnels';
import { partInput, partMeshes, partStamps, partVolumes } from './parts';
import { culvertMouths } from '../roads/mouths';
import { streetLamps } from '../roads/lamps';

/** A portal's box reaches this far from it. */
const REACH_M = 20;

/** Whether a world point lies in a square, half-open. */
export type Owns = (x: number, y: number) => boolean;

/** Words of a floor segment: its ends in world units, their heights, and its half-width in metres: out to a deck's parapets, to a bore's walls. */
export const FLOOR = 7;

/**
 * The floors that stand in an OWNER node's square: each segment of a deck's road and of a tunnel's road
 * that comes within REACH_M of it, as FLOOR words. With the ground plan (`plan`) a chain that draws as parts has the
 * floors of its parts. Null without floors.
 */
export function floorsOf(list: Feature[], tile: Node, node: Node, plan = false): Float64Array<ArrayBuffer> | null {
    if (node.level !== OWNER) return null;
    const side = 2 ** (30 - node.level), x0 = node.x * side, y0 = node.y * side, m = REACH_M / metresPerUnit(y0 + side / 2), out: number[] = [];
    const { lines } = features(list, tile), parts = plan ? partStamps(list, tile, lines) : undefined, volumes = parts && partVolumes(list, tile, node);
    for (const line of lines) {
        if ((line.kind !== BRIDGE && line.kind !== TUNNEL) || parts?.built.has(line)) continue;
        const lift = line.kind === BRIDGE ? LIFT_M : 0;
        for (let v = 0; v + 1 < line.z.length; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4);
            if (Math.max(ax, bx) < x0 - m || Math.min(ax, bx) > x0 + side + m || Math.max(ay, by) < y0 - m || Math.min(ay, by) > y0 + side + m) continue;
            out.push(ax, ay, bx, by, line.z[v] + lift, line.z[v + 1] + lift, line.kind === BRIDGE ? deckHalf(line) : boreHalf(line));
        }
    }
    if (volumes) out.push(...volumes.floors);
    return out.length ? new Float64Array(out) : null;
}

/**
 * Words of a box in which the ground does not draw (`BOX` of obc-trailview-parts): the middle of its start in world units,
 * its unit way in, and its depth, half-width, roof, floor, rise, spring and the slopes of its two ends. A portal's box is
 * its `portalBox` with the spring at its roof and slopes 0.
 */
export const BOX = 12;

/**
 * The boxes of the tunnels' portals within REACH_M of a node's square, where the terrain draws nothing; with the ground plan
 * (`plan`), those of the parts near it for the chains that draw as parts. Null without any.
 */
export function portalBoxes(list: Feature[], tile: Node, node: Node, plan = false): Float64Array<ArrayBuffer> | null {
    const side = 2 ** (30 - node.level), x0 = node.x * side, y0 = node.y * side, m = REACH_M / metresPerUnit(y0 + side / 2), out: number[] = [];
    const { lines } = features(list, tile), twin = twins(lines), parts = plan ? partStamps(list, tile, lines) : undefined;
    for (const line of lines) {
        if (line.kind !== TUNNEL || !line.abut || parts?.built.has(line)) continue;
        for (const end of [0, 1]) {
            const v = end ? line.z.length - 1 : 0, w = end ? v - 1 : 1, x = line.xy[2 * v], y = line.xy[2 * v + 1], pair = twin.get(line)?.[end];
            if (!line.abut[end] || pair === null || x < x0 - m || x >= x0 + side + m || y < y0 - m || y >= y0 + side + m) continue;
            // A portal over two tracks: its middle moves to the left of the way in, as `portal` of tunnels.ts moves it.
            const dx = line.xy[2 * w] - x, dy = line.xy[2 * w + 1] - y, l = Math.hypot(dx, dy) || 1, shift = (pair?.shift ?? 0) / metresPerUnit(y);
            const [depth, half, roof, floor, rise] = portalBox(line, v, end ? 0 : line.z.length - 1, pair?.widen);
            out.push(x + dy / l * shift, y - dx / l * shift, dx / l, dy / l, depth, half, roof, floor, rise, roof, 0, 0);
        }
    }
    const volumes = parts && partVolumes(list, tile, node);
    if (volumes) out.push(...volumes.boxes);
    return out.length ? new Float64Array(out) : null;
}

/** Meshes of one reach: the structures, and the anchored mesh. */
export interface Meshes { structures: StructureMesh | null; anchored: AnchoredMesh | null }
/**
 * A node's meshes: those that draw at any distance, those that draw only near the eye, and the anchors of
 * both; and the models on the surface of its kinds, as `packPoints` packs them, the model the number of
 * POINT_MODELS, the last `pointsOff` of them in a part, which the ground plan does not draw.
 */
export interface NodeMeshes { far: Meshes; near: Meshes; anchors: ArrayBuffer | null; points: Uint32Array<ArrayBuffer> | null; pointsOff?: number }

/**
 * The meshes of a node of the OWNER level from the features of its feature tile: the structures, then the kinds of the
 * registry. With the ground plan (`plan`) the plan's mesh draws the culverts' mouths (plan/piece.wgsl): none stands here;
 * a chain that draws as parts has today's tunnel in the range that only today's app draws, and the parts in the range
 * that only the ground plan draws.
 */
export function structureMeshes(list: Feature[], tile: Node, node: Node, plan = false): NodeMeshes {
    const none = { structures: null, anchored: null };
    if (node.level !== OWNER) return { far: none, near: none, anchors: null, points: null };
    const side = 2 ** (30 - node.level), x0 = node.x * side, y0 = node.y * side;
    const owns: Owns = (x, y) => x >= x0 && x < x0 + side && y >= y0 && y < y0 + side;
    const words: number[] = [], far = { mesh: new MeshBuilder(), anchored: new AnchoredBuilder(words) }, near = { mesh: new MeshBuilder(), anchored: new AnchoredBuilder(words) };
    const shapes = features(list, tile), { lines } = shapes;
    far.mesh.bridge(() => bridges(far.mesh, list, tile, lines, owns));
    const parts = plan ? partStamps(list, tile, lines) : undefined;
    if (parts) {
        tunnels(far.mesh, lines, owns, line => !parts.built.has(line));
        far.mesh.offPlan(() => tunnels(far.mesh, lines, owns, line => parts.built.has(line)));
        const input = partInput(list, tile, node), mesh = input && partMeshes(input);
        if (mesh) far.mesh.planOnly(() => far.mesh.append(new Uint32Array(mesh.vertices), mesh.indices));
    } else tunnels(far.mesh, lines, owns);
    galleries(far.mesh, lines, owns);
    rails(far.mesh, far.anchored, list, tile, lines, owns, [x0, y0, x0 + side, y0 + side]);
    const points: Point[] = [];
    let kind = '';
    const place = ({ model, tint = 1, ...at }: Placement) => points.push({ ...at, tint, model: POINT_MODELS.indexOf(`${kind}/${model}`) });
    const out: Build = { far, near, owns, place, shapes, cores: coresOf(shapes), taken: [], cornered: (x, y) => !!cornerAt(lines, x, y) };
    for (const feature of list) {
        kind = feature.kind;
        KINDS.get(kind)?.build(placed(feature, tile), out);
    }
    kind = 'road';
    streetLamps(out);
    kind = 'stream';
    if (!plan) culvertMouths(out, shapes);
    const meshes = ({ mesh, anchored }: typeof far): Meshes => ({ structures: mesh.mesh(), anchored: anchored.mesh() });
    const kept = points.filter(p => p.model >= 0), off = new Set(parts ? kept.filter(p => parts.taken.some(ring => insideRings([ring], p.x, p.y))) : []);
    const packed = points.length ? packPoints(off.size ? [...kept.filter(p => !off.has(p)), ...off] : kept) : null;
    return { far: meshes(far), near: meshes(near), anchors: words.length ? anchorBytes(words) : null, points: packed, ...(off.size ? { pointsOff: off.size } : {}) };
}
