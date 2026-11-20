// The tunnel parts of a node, from the part records of its feature tile (`tunnel-end`, `tunnel-tube`): their meshes,
// the boxes in which the ground does not draw, the floor segments and the rings that the parts take from the ground.
// host/obc-trailview-parts builds them, as WASM that this module inlines as a data: URL, so it loads the same way in a
// stamp worker and in the Node bake.
import wasm from './pkg/obc_trailview_parts_bridge_bg.wasm?url&inline';
import { built as builtKinds, footprints as footprintsOf, initSync, meshes, volumes } from './pkg/obc_trailview_parts_bridge';
import type { Feature } from '../stream/features';
import { classPaint, metresPerUnit, TUNNEL, worldPoint, type Line, type PartStamps } from '../terrain/bins';
import type { Node } from '../terrain/lod';
import type { StructureMesh } from './mesh';

let ready = false;

/** Starts the bridge on the first call. */
function bridge(): void {
    if (ready) return;
    initSync({ module: Uint8Array.from(atob(wasm.slice(wasm.indexOf(',') + 1)), c => c.charCodeAt(0)) });
    ready = true;
}

/** The part records of each tile as the bridge's JSON holds them, made once; null for a tile without an end. */
const records = new WeakMap<Feature[], string | null>();

function recordsOf(list: Feature[], tile: Node): string | null {
    let known = records.get(list);
    if (known !== undefined) return known;
    const ends: object[] = [], chords: object[] = [];
    for (const { kind, tags, parts } of list) {
        for (const { xy, z } of kind === 'tunnel-end' || kind === 'tunnel-tube' ? parts : []) {
            if (!z) continue;
            if (kind === 'tunnel-end') ends.push({ at: worldPoint(tile, xy, 0), z: z[0], tags });
            else chords.push({ a: [...worldPoint(tile, xy, 0), z[0]], b: [...worldPoint(tile, xy, 1), z[1]], tags, ...classPaint(tags.class, tags.surface) });
        }
    }
    known = ends.length ? `"ends":${JSON.stringify(ends)},"chords":${JSON.stringify(chords)}` : null;
    records.set(list, known);
    return known;
}

/**
 * The input of the bridge for `node` of its feature tile: the node's square and the tile's part records in world
 * units, each chord with the surface code and the lines of a way of its class and surface; null for a tile without a
 * `tunnel-end`, which has no part.
 */
export function partInput(list: Feature[], tile: Node, node: Node): string | null {
    const side = 2 ** (30 - node.level), parts = recordsOf(list, tile);
    return parts && `{"square":${JSON.stringify([node.x * side, node.y * side, (node.x + 1) * side, (node.y + 1) * side])},${parts}}`;
}

/** The meshes of the parts that the input's square draws; null with none. */
export function partMeshes(input: string): StructureMesh | null {
    bridge();
    const out = meshes(input), n = out[0];
    return out.length > 1 ? { vertices: out.slice(1, 1 + n).buffer, indices: out.slice(1 + n) } : null;
}

/** The boxes (`BOX` of obc-trailview-parts, 12 words each) and the floor segments (`FLOOR` words) of the parts near the input's square. */
export function partVolumes(input: string): { boxes: Float64Array<ArrayBuffer>; floors: Float64Array<ArrayBuffer> } {
    bridge();
    const out = volumes(input), n = out[0];
    return { boxes: out.slice(1, 1 + n), floors: out.slice(1 + n) };
}

/** Whether the kinds of a tag `parts` or `ends`, separated by a space, are all built. */
export function built(parts: string): boolean {
    bridge();
    return builtKinds(parts);
}

/** The ring that each built part of the input takes from the ground, its points x, y in world units. */
export function footprints(input: string): number[][] {
    bridge();
    const out = footprintsOf(input), rings: number[][] = [];
    for (let at = 1; rings.length < out[0]; at += 1 + 2 * out[at]) rings.push(Array.from(out.subarray(at + 1, at + 1 + 2 * out[at])));
    return rings;
}

/** The cover over a part starts this far past its collar (`need` of obc-trailview-parts). */
const COVER_START_M = 1;

const stamped = new WeakMap<Feature[], PartStamps>();

/** The metres along a line. */
const lengthOf = (line: Line) => line.z.slice(1).reduce((sum, _, v) => sum + Math.hypot(line.xy[2 * v + 2] - line.xy[2 * v], line.xy[2 * v + 3] - line.xy[2 * v + 1]) * metresPerUnit(line.xy[2 * v + 3]), 0);

/**
 * What the parts of a tile stamp with the ground plan (`PartStamps` of bins.ts), made once. A tunnel line of a chain that
 * draws as parts stamps no cover from each opening of its chain (the end vertex of its way, `shift` metres before it) to
 * the start of its part's cover, `collar` + COVER_START_M past the opening, along the chain into its next line.
 */
export function partStamps(list: Feature[], tile: Node, lines: Line[]): PartStamps {
    const known = stamped.get(list);
    if (known) return known;
    const input = partInput(list, tile, tile);
    const out: PartStamps = { built: new Set(), stops: new Map(), taken: input ? footprints(input) : [] };
    stamped.set(list, out);
    if (!input) return out;
    const vertex = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
    // The ends of the built tunnel lines at each vertex.
    const at = new Map<string, [Line, number][]>();
    for (const line of lines) {
        if (line.kind !== TUNNEL || !line.parts || !built(line.parts)) continue;
        out.built.add(line);
        for (const end of [0, 1]) {
            const key = vertex(line, end ? line.z.length - 1 : 0);
            (at.get(key) ?? at.set(key, []).get(key)!).push([line, end]);
        }
    }
    const stop = (line: Line, end: number, metres: number) => {
        for (;;) {
            const stops = out.stops.get(line) ?? [0, 0];
            stops[end] = Math.max(stops[end], metres);
            out.stops.set(line, stops);
            metres -= lengthOf(line);
            const next = at.get(vertex(line, end ? 0 : line.z.length - 1))?.filter(([other]) => other !== line);
            if (metres <= 0 || next?.length !== 1) return;
            [line, end] = next[0];
        }
    };
    for (const { kind, tags } of list) {
        if (kind !== 'tunnel-end') continue;
        for (const [way, shift] of [[tags.way, tags.shift], [tags.mate_way, tags.mate_shift]]) {
            // `way` is in the frame of the ground plan, 16 units to a world unit.
            const [x, y] = way?.split(' ').map(Number) ?? [];
            for (const [line, end] of at.get(`${x / 16},${y / 16}`) ?? []) stop(line, end, Number(shift) + Number(tags.collar) + COVER_START_M);
        }
    }
    return out;
}
