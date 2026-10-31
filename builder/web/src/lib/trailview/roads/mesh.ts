// The corridor mesh of an L15 owner from its feature tile: one piece per L16 child, or more where a child
// holds more vertices than 16-bit indices reach. A segment belongs to the child that holds its first station.
import type { Feature } from '../stream/features';
import { features, junctions, metresPerUnit, ROAD, type Line, type Node, type Shapes } from '../terrain/bins';
import { halfAt, partnersOf } from '../terrain/corners';
import { CHILD, size } from '../terrain/lod';
import { townOf } from '../terrain/town';
import { roadside } from './details';
import type { GroundAt } from './ground';
import { PieceBuilder, type RoadPiece } from './piece';
import { corridorLine } from './section';
import { allowed, barriersOf, mouthsOn, standingOf, townish, zonesOf, type Barriers } from './standing';
import { farStations, reachOf, stationsOf, type Station } from './stations';
import { baseOf, cap, liftOf, prioOf, sweep, type Run } from './sweep';
import { waysOf } from './ways';

/** The ground beside a road stands within this many metres of its plane: a piece's heights reach this far past its vertices. */
const GROUND_M = 15;

export interface RoadMesh { pieces: RoadPiece[] }

/**
 * A line's corridor as the owners of a tile share it: the stations of its far range, whether each end is a dead end,
 * and per station the metres to its nearest junction's mouth, and to that or a town; the barriers beside it.
 */
type Planned = Run & { far: number[]; ends: [boolean, boolean]; mouths: Float64Array; towns: Float64Array; barriers: Barriers };

const plans = new WeakMap<Shapes['lines'], Planned[]>();

/** The lines of a tile as runs, made once. */
function planOf({ lines, buildings, barriers: mapped }: Shapes): Planned[] {
    const known = plans.get(lines);
    if (known) return known;
    const ways = waysOf(lines), shared = junctions(lines), partners = partnersOf(lines), town = townOf(buildings), barriers = barriersOf(mapped);
    const open = (line: Line, v: number) => !shared.has(`${line.xy[2 * v]},${line.xy[2 * v + 1]}`);
    const runs = lines.flatMap((line, id) => {
        if (!corridorLine(line) || line.z.length < 2) return [];
        const stations = stationsOf(line, v => halfAt(line, v, partners)), joins = mouthsOn(line, shared), ends: [boolean, boolean] = [open(line, 0), open(line, line.z.length - 1)], last = stations.length - 1;
        // A detail also ramps down before a dead end, where its section would stand open.
        const inMouth = (j: number) => (ends[0] && j === 0) || (ends[1] && j === last) || joins.some(({ x, y, gap }) => Math.hypot(x - stations[j].x, y - stations[j].y) * metresPerUnit(y) < gap);
        const { railed, walled } = allowed(line), mapped = barriers.segments.length > 0 && line.kind === ROAD, mouths = zonesOf(railed || walled || mapped ? stations : [], inMouth);
        const towns = railed ? zonesOf(stations, j => inMouth(j) || townish(town, stations[j].x, stations[j].y)) : mouths;
        return [{ id, line, stations, reach: reachOf(stations), lift: liftOf(line, id), prio: prioOf(line), base: 0, ways, far: farStations(line, stations, size(CHILD)), ends, mouths, towns, barriers }];
    });
    plans.set(lines, runs);
    return runs;
}

/** The corridor mesh of an owner. */
export function roadMesh(list: Feature[], tile: Node, owner: Node, ground: GroundAt = () => NaN): RoadMesh {
    const runs = planOf(features(list, tile)), side = size(CHILD), pieces: PieceBuilder[][] = [[], [], [], []];
    const x0 = owner.x * 2 * side, y0 = owner.y * 2 * side;
    const owns = ({ x, y }: Station) => x >= x0 && x < x0 + 2 * side && y >= y0 && y < y0 + 2 * side;
    /** The piece of the child that holds a station, with room for its faces. */
    const pieceAt = ({ x, y }: Station) => {
        const i = Math.floor((x - x0) / side), j = Math.floor((y - y0) / side), list = pieces[j * 2 + i];
        const last = list[list.length - 1];
        return last?.roomy ? last : list[list.push(new PieceBuilder(x0 + (i + 0.5) * side, y0 + (j + 0.5) * side, side / 2)) - 1];
    };
    for (const planned of runs) {
        const { stations, far, ends, line } = planned, first = stations.find(owns);
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
        for (const [end, j, dir] of [[0, 0, -1], [1, stations.length - 1, 1]] as const) {
            if (!ends[end] || !owns(stations[j])) continue;
            const piece = pieceAt(stations[j]);
            cap(piece, piece.near, run, j, dir, false);
            cap(piece, piece.far, run, j, dir, true);
        }
        if (run.mouths.length) for (const way of [1, -1] as const) roadside(run, way, standingOf(line, stations, run.reach, way, ground, run.mouths, run.towns, run.barriers), owns, pieceAt);
    }
    return { pieces: pieces.flat().map(piece => piece.piece(GROUND_M)) };
}

/** The heap per station of a plan, and per segment with the tile's shapes, fitted to the V8 heap of fixture tiles. */
const STATION_BYTES = 256, SEGMENT_BYTES = 430;

/** The heap that hangs from a tile's features once its plan stands. */
export function planBytes(list: Feature[], tile: Node): number {
    return planOf(features(list, tile)).reduce((sum, { stations, line }) => sum + STATION_BYTES * stations.length + SEGMENT_BYTES * line.z.length, 0);
}

/** Bytes that a mesh holds. */
export const meshBytes = ({ pieces }: RoadMesh) => pieces.reduce((sum, { vertices, indices }) => sum + vertices.byteLength + indices.byteLength, 0);
