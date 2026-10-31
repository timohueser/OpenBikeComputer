// The corridor mesh of an L15 owner from its feature tile: one piece per L16 child, or more where a child
// holds more vertices than 16-bit indices reach. A segment belongs to the child that holds its first station.
import type { Feature } from '../stream/features';
import { features, junctions as sharedPoints, type Line, type Node } from '../terrain/bins';
import { halfAt, partnersOf } from '../terrain/corners';
import { CHILD, size } from '../terrain/lod';
import { cornerFaces, mouthFaces } from './junction-faces';
import { planJunctions, type Junctions } from './junctions';
import { PieceBuilder, type RoadPiece } from './piece';
import { corridorLine, frays } from './section';
import { farStations, reachOf, stationIndex, stationsOf } from './stations';
import { baseOf, cap, liftOf, prioOf, sweep, type Run } from './sweep';
import { waysOf } from './ways';

/** The ground beside a road stands within this many metres of its plane: a piece's heights reach this far past its vertices. */
const GROUND_M = 15;

export interface RoadMesh { pieces: RoadPiece[] }

/** A line's corridor as the owners of a tile share it: the stations of its far range, and its round ends, each with its direction along the line. */
type Planned = Run & { far: number[]; caps: { j: number; dir: 1 | -1 }[] };

/** A tile's corridors and its junctions, as the owners of the tile share them. */
export interface TilePlan { runs: Planned[]; junctions: Junctions }

const plans = new WeakMap<Line[], TilePlan>();

/** The plan of a tile's lines, made once. */
export function planOf(lines: Line[]): TilePlan {
    const known = plans.get(lines);
    if (known) return known;
    const ways = waysOf(lines), shared = sharedPoints(lines), partners = partnersOf(lines);
    const open = (line: Line, v: number) => !shared.has(`${line.xy[2 * v]},${line.xy[2 * v + 1]}`);
    const runs: Planned[] = lines.flatMap((line, id) => {
        if (!corridorLine(line) || line.z.length < 2) return [];
        return [{ id, line, stations: stationsOf(line, v => halfAt(line, v, partners)), reach: new Float64Array(0), lift: liftOf(line, id), prio: prioOf(line), base: 0, ways, far: [], caps: [] }];
    });
    const junctions = planJunctions(lines, runs);
    for (const run of runs) {
        const { line, stations } = run, last = line.z.length - 1;
        run.reach = reachOf(stations);
        run.far = farStations(line, stations, size(CHILD));
        if (open(line, 0)) run.caps.push({ j: 0, dir: -1 });
        if (open(line, last)) run.caps.push({ j: stations.length - 1, dir: 1 });
    }
    // A way of soil frays into the ground at its end on a road, as at a dead end; a paved one meets the kerb or the shoulder square.
    for (const { way: { run, step }, at } of junctions.ends) if (frays(run.line)) (run as Planned).caps.push({ j: stationIndex(run.stations, at), dir: -step as 1 | -1 });
    const plan = { runs, junctions };
    plans.set(lines, plan);
    return plan;
}

/** The corridor mesh of an owner. */
export function roadMesh(list: Feature[], tile: Node, owner: Node): RoadMesh {
    const { runs, junctions } = planOf(features(list, tile).lines), side = size(CHILD), pieces: PieceBuilder[][] = [[], [], [], []];
    const x0 = owner.x * 2 * side, y0 = owner.y * 2 * side;
    const owns = ({ x, y }: { x: number; y: number }) => x >= x0 && x < x0 + 2 * side && y >= y0 && y < y0 + 2 * side;
    const listAt = ({ x, y }: { x: number; y: number }) => pieces[Math.floor((y - y0) / side) * 2 + Math.floor((x - x0) / side)];
    /** A new piece of the child that holds a point. */
    const newPiece = (at: { x: number; y: number }) => {
        const i = Math.floor((at.x - x0) / side), j = Math.floor((at.y - y0) / side), list = listAt(at);
        return list[list.push(new PieceBuilder(x0 + (i + 0.5) * side, y0 + (j + 0.5) * side, side / 2)) - 1];
    };
    /** The piece of the child that holds a point, with room for a cross-section. */
    const pieceAt = (at: { x: number; y: number }) => {
        const list = listAt(at), last = list[list.length - 1];
        return last?.roomy ? last : newPiece(at);
    };
    /** A junction's faces, whole in one piece: a dense line gives them any number of stations. */
    const facesAt = (at: { x: number; y: number }, make: (piece: PieceBuilder) => void) => {
        if (!pieceAt(at).attempt(make)) make(newPiece(at));
    };
    for (const planned of runs) {
        const { stations, far, caps, line } = planned, first = stations.find(owns);
        if (!first) continue;
        const run = { ...planned, base: baseOf(line, first) };
        for (let j = 0; j + 1 < stations.length; j++) {
            if (owns(stations[j])) {
                const piece = pieceAt(stations[j]);
                sweep(piece, piece.near, run, j, j + 1, false);
            }
        }
        for (let k = 0; k + 1 < far.length; k++) {
            if (owns(stations[far[k]])) {
                const piece = pieceAt(stations[far[k]]);
                sweep(piece, piece.far, run, far[k], far[k + 1], true);
            }
        }
        for (const { j, dir } of caps) {
            if (!owns(stations[j])) continue;
            const piece = pieceAt(stations[j]);
            cap(piece, piece.near, run, j, dir, false);
            cap(piece, piece.far, run, j, dir, true);
        }
    }
    // A junction's faces go to the child that holds its point, so one owner of the tiles that hold them draws them.
    for (const corner of junctions.corners) if (owns(corner.meeting)) facesAt(corner.meeting, piece => cornerFaces(piece, corner));
    for (const mouth of junctions.mouths) if (owns(mouth.left.meeting)) facesAt(mouth.left.meeting, piece => mouthFaces(piece, mouth));
    return { pieces: pieces.flat().map(piece => piece.piece(GROUND_M)) };
}

/** The heap per station of a plan, per segment with the tile's shapes, and per corner, mouth and path end, fitted to the V8 heap of fixture tiles. */
const STATION_BYTES = 240, SEGMENT_BYTES = 430, FACE_BYTES = 3500;

/** The heap that hangs from a tile's features once its plan stands. */
export function planBytes(list: Feature[], tile: Node): number {
    const { runs, junctions: { corners, mouths, ends } } = planOf(features(list, tile).lines);
    return runs.reduce((sum, { stations, line }) => sum + STATION_BYTES * stations.length + SEGMENT_BYTES * line.z.length, FACE_BYTES * (corners.length + mouths.length + ends.length));
}

/** Bytes that a mesh holds. */
export const meshBytes = ({ pieces }: RoadMesh) => pieces.reduce((sum, { vertices, indices }) => sum + vertices.byteLength + indices.byteLength, 0);
