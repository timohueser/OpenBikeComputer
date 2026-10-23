import type { Feature } from '../stream/features';
import { BRIDGE, features, metresPerUnit, TUNNEL, type Node } from '../terrain/bins';
import { bridges } from './bridges';
import { galleries } from './galleries';
import { MeshBuilder, type StructureMesh } from './mesh';
import { portalBox, tunnels } from './tunnels';

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

/** Words of a floor segment: its ends in world units, their heights, and its half-width in metres. */
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
            out.push(ax, ay, bx, by, line.z[v] + lift, line.z[v + 1] + lift, line.h);
        }
    }
    return out.length ? new Float64Array(out) : null;
}

/** Words of a portal's box: its facade's middle in world units, its unit way into the tunnel, its depth and its half-width in metres. */
export const BOX = 6;

/** The boxes of the tunnels' portals within REACH_M of a node's square, where the terrain draws nothing; null without any. */
export function portalBoxes(list: Feature[], tile: Node, node: Node): Float64Array<ArrayBuffer> | null {
    const side = 2 ** (30 - node.level), x0 = node.x * side, y0 = node.y * side, m = REACH_M / metresPerUnit(y0 + side / 2), out: number[] = [];
    for (const line of features(list, tile).lines) {
        if (line.kind !== TUNNEL || !line.abut) continue;
        for (const end of [0, 1]) {
            const v = end ? line.z.length - 1 : 0, w = end ? v - 1 : 1, x = line.xy[2 * v], y = line.xy[2 * v + 1];
            if (!line.abut[end] || x < x0 - m || x >= x0 + side + m || y < y0 - m || y >= y0 + side + m) continue;
            const dx = line.xy[2 * w] - x, dy = line.xy[2 * w + 1] - y, l = Math.hypot(dx, dy) || 1;
            out.push(x, y, dx / l, dy / l, ...portalBox(line));
        }
    }
    return out.length ? new Float64Array(out) : null;
}

/** The structure mesh of a node of the OWNER level from the features of its feature tile, null without structures. */
export function structureMesh(list: Feature[], tile: Node, node: Node): StructureMesh | null {
    if (node.level !== OWNER) return null;
    const side = 2 ** (30 - node.level), x0 = node.x * side, y0 = node.y * side;
    const owns: Owns = (x, y) => x >= x0 && x < x0 + side && y >= y0 && y < y0 + side;
    const b = new MeshBuilder(), { lines } = features(list, tile);
    bridges(b, list, tile, lines, owns);
    tunnels(b, lines, owns);
    galleries(b, lines, owns);
    return b.mesh();
}
