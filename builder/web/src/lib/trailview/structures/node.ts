import type { Feature } from '../stream/features';
import { features, metresPerUnit, type Node } from '../terrain/bins';
import { bridges } from './bridges';
import { galleries } from './galleries';
import { MeshBuilder, type StructureMesh } from './mesh';
import { portals } from './portals';

/** Structures reach this far past the square of their node: the node draws its own part of them. */
const REACH_M = 40;

/** The structure mesh of a node from the features of its feature tile, null without structures. */
export function structureMesh(list: Feature[], tile: Node, node: Node): StructureMesh | null {
    const shift = 22 - node.level, side = 256 * 2 ** shift, x0 = node.x * side, y0 = node.y * side, m = REACH_M / metresPerUnit(y0 + side / 2);
    const box: [number, number, number, number] = [x0 - m, y0 - m, x0 + side + m, y0 + side + m];
    const b = new MeshBuilder(), { lines } = features(list, tile);
    bridges(b, list, tile, lines, box);
    portals(b, lines, box);
    galleries(b, lines, box);
    return b.mesh();
}
