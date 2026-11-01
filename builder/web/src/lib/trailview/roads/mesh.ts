// The corridor mesh of an L15 owner from its feature tile: one piece per L16 child, or more where a child
// holds more vertices than 16-bit indices reach. A segment belongs to the child that holds its first station.
import type { Feature } from '../stream/features';
import { features, junctions as sharedPoints, metresPerUnit, ROAD, type Line, type Node, type Shapes } from '../terrain/bins';
import { halfAt, partnersOf } from '../terrain/corners';
import { CHILD, DETAIL, size } from '../terrain/lod';
import { townOf } from '../terrain/town';
import { roadside } from './details';
import type { GroundAt } from './ground';
import { cornerFaces, mouthFaces } from './junction-faces';
import { planJunctions, type Junctions } from './junctions';
import { PieceBuilder, type RoadPiece } from './piece';
import { corridorLine, frays } from './section';
import { allowed, barriersOf, mouthsOn, standingOf, townish, zonesOf, type Barriers } from './standing';
import { shoreBytes, shoreOf } from './shore';
import { farStations, reachOf, stationIndex, stationsOf, type Station } from './stations';
import { baseOf, cap, liftOf, prioOf, stopsOf, sweep, type Run } from './sweep';
import { tracks } from './tracks';
import { waysOf } from './ways';

/** The ground beside a road stands within this many metres of its plane: a piece's heights reach this far past its vertices. */
const GROUND_M = 15;

export interface RoadMesh { pieces: RoadPiece[] }

/**
 * A line's corridor as the owners of a tile share it: the stations of its far range, its round ends, each with its
 * direction along the line, and per station the metres to its nearest junction's mouth, and to that or a town; the barriers beside it.
 */
type Planned = Run & { far: number[]; caps: { j: number; dir: 1 | -1 }[]; mouths: Float64Array; towns: Float64Array; barriers: Barriers };

/** A tile's corridors and its junctions, as the owners of the tile share them. */
export interface TilePlan { runs: Planned[]; junctions: Junctions }

const plans = new WeakMap<Line[], TilePlan>();

/** The plan of a tile's shapes, made once. */
export function planOf(shapes: Pick<Shapes, 'lines' | 'lakes' | 'seas' | 'areas' | 'buildings' | 'barriers'>): TilePlan {
    const { lines, buildings, barriers: mapped } = shapes, known = plans.get(lines);
    if (known) return known;
    const shore = shoreOf(shapes), ways = waysOf(lines), shared = sharedPoints(lines), partners = partnersOf(lines), town = townOf(buildings), barriers = barriersOf(mapped);
    const open = (line: Line, v: number) => !shared.has(`${line.xy[2 * v]},${line.xy[2 * v + 1]}`);
    const runs: Planned[] = lines.flatMap((line, id) => {
        if (!corridorLine(line) || line.z.length < 2) return [];
        return [{ id, line, stations: stationsOf(line, v => halfAt(line, v, partners)), reach: new Float64Array(0), lift: liftOf(line, id), prio: prioOf(line), base: 0, ways, shore, far: [], caps: [], mouths: new Float64Array(0), towns: new Float64Array(0), barriers }];
    });
    const junctions = planJunctions(lines, runs);
    for (const run of runs) {
        const { line, stations } = run, last = stations.length - 1, first = open(line, 0), end = open(line, line.z.length - 1), joins = mouthsOn(line, shared);
        run.reach = reachOf(stations);
        if (shore) run.stops = stopsOf(line, stations, shore);
        run.far = farStations(line, stations, size(CHILD));
        if (first) run.caps.push({ j: 0, dir: -1 });
        if (end) run.caps.push({ j: last, dir: 1 });
        // A detail also ramps down before a dead end, where its section would stand open.
        const inMouth = (j: number) => (first && j === 0) || (end && j === last) || joins.some(({ x, y, gap }) => Math.hypot(x - stations[j].x, y - stations[j].y) * metresPerUnit(y) < gap);
        const { railed, walled } = allowed(line);
        run.mouths = zonesOf(railed || walled || (barriers.segments.length > 0 && line.kind === ROAD) ? stations : [], inMouth);
        run.towns = railed ? zonesOf(stations, j => inMouth(j) || townish(town, stations[j].x, stations[j].y)) : run.mouths;
    }
    // A way of soil frays into the ground at its end on a road, as at a dead end; a paved one meets the kerb or the shoulder square.
    for (const { way: { run, step }, at } of junctions.ends) if (frays(run.line)) (run as Planned).caps.push({ j: stationIndex(run.stations, at), dir: -step as 1 | -1 });
    const plan = { runs, junctions };
    plans.set(lines, plan);
    return plan;
}

/** The corridor mesh of an owner. */
export function roadMesh(list: Feature[], tile: Node, owner: Node, ground: GroundAt = () => NaN): RoadMesh {
    const { runs, junctions } = planOf(features(list, tile)), side = size(CHILD), pieces: PieceBuilder[][] = [[], [], [], []];
    const x0 = owner.x * 2 * side, y0 = owner.y * 2 * side;
    const owns = ({ x, y }: { x: number; y: number }) => x >= x0 && x < x0 + 2 * side && y >= y0 && y < y0 + 2 * side;
    const close = new Map<number, PieceBuilder[]>(), cell = size(DETAIL);
    /** The piece of the details of a station that the owner holds, with room for its faces. */
    const detailAt = (s: Station) => {
        if (!owns(s)) return undefined;
        const [i, j] = [Math.floor(s.x / cell), Math.floor(s.y / cell)], list = close.get(i * 2 ** 20 + j) ?? close.set(i * 2 ** 20 + j, []).get(i * 2 ** 20 + j)!;
        const last = list[list.length - 1];
        return last?.roomy ? last : list[list.push(new PieceBuilder((i + 0.5) * cell, (j + 0.5) * cell, DETAIL, true)) - 1];
    };
    const listAt = ({ x, y }: { x: number; y: number }) => pieces[Math.floor((y - y0) / side) * 2 + Math.floor((x - x0) / side)];
    /** A new piece of the child that holds a point. */
    const newPiece = (at: { x: number; y: number }) => {
        const i = Math.floor((at.x - x0) / side), j = Math.floor((at.y - y0) / side), list = listAt(at);
        return list[list.push(new PieceBuilder(x0 + (i + 0.5) * side, y0 + (j + 0.5) * side)) - 1];
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
        tracks(run, detailAt);
        if (run.mouths.length) for (const way of [1, -1] as const) roadside(run, way, standingOf(line, stations, run.reach, way, ground, run.mouths, run.towns, run.barriers), owns, pieceAt);
    }
    // A junction's faces go to the child that holds its point, so one owner of the tiles that hold them draws them.
    for (const corner of junctions.corners) if (owns(corner.meeting)) facesAt(corner.meeting, piece => cornerFaces(piece, corner));
    for (const mouth of junctions.mouths) if (owns(mouth.left.meeting)) facesAt(mouth.left.meeting, piece => mouthFaces(piece, mouth));
    return { pieces: [...pieces.flat(), ...[...close.values()].flat()].map(piece => piece.piece(GROUND_M)) };
}

/** The heap per station of a plan, per segment with the tile's shapes, and per corner, mouth and path end, fitted to the V8 heap of fixture tiles. */
const STATION_BYTES = 256, SEGMENT_BYTES = 430, FACE_BYTES = 3500;

/** The heap that hangs from a tile's features once its plan stands. */
export function planBytes(list: Feature[], tile: Node): number {
    const shapes = features(list, tile), { runs, junctions: { corners, mouths, ends } } = planOf(shapes);
    return shoreBytes(shapes) + runs.reduce((sum, { stations, line }) => sum + STATION_BYTES * stations.length + SEGMENT_BYTES * line.z.length, FACE_BYTES * (corners.length + mouths.length + ends.length));
}

/** Bytes that a mesh holds. */
export const meshBytes = ({ pieces }: RoadMesh) => pieces.reduce((sum, { vertices, indices }) => sum + vertices.byteLength + indices.byteLength, 0);
