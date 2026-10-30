// The corridor mesh of an L15 owner from its feature tile: one piece per L16 child, or more where a child
// holds more vertices than 16-bit indices reach. A segment belongs to the child that holds its first station.
import type { Feature } from '../stream/features';
import { features, junctions, type Line, type Node } from '../terrain/bins';
import { halfAt, partnersOf } from '../terrain/corners';
import { PieceBuilder, type RoadPiece } from './piece';
import { corridorLine } from './section';
import { farStations, reachOf, stationsOf, type Station } from './stations';
import { baseOf, cap, liftOf, prioOf, sweep, type Run } from './sweep';
import { waysOf, type Ways } from './ways';

/** The level of the owners, and of their children. */
export const OWNER = 15, CHILD = 16;
/** The ground beside a road stands within this many metres of its plane: a piece's heights reach this far past its vertices. */
const GROUND_M = 15;

export interface RoadMesh { pieces: RoadPiece[] }

/** What the owners of a tile share. */
interface Plan { runs: (Run & { far: number[]; ends: [boolean, boolean] })[]; ways: Ways }

const plans = new WeakMap<Line[], Plan>();

/** The lines of a tile as runs, made once: its owners share them. */
function planOf(lines: Line[]): Plan {
    const known = plans.get(lines);
    if (known) return known;
    const ways = waysOf(lines), shared = junctions(lines), partners = partnersOf(lines);
    const open = (line: Line, v: number) => !shared.has(`${line.xy[2 * v]},${line.xy[2 * v + 1]}`);
    const runs = lines.flatMap((line, id) => {
        if (!corridorLine(line) || line.z.length < 2) return [];
        const stations = stationsOf(line, v => halfAt(line, v, partners));
        return [{ id, line, stations, reach: reachOf(stations), lift: liftOf(line, id), prio: prioOf(line), base: 0, ways, far: farStations(line, stations), ends: [open(line, 0), open(line, line.z.length - 1)] as [boolean, boolean] }];
    });
    const plan = { runs, ways };
    plans.set(lines, plan);
    return plan;
}

/** The corridor mesh of an owner. */
export function roadMesh(list: Feature[], tile: Node, owner: Node): RoadMesh {
    const { runs } = planOf(features(list, tile).lines), side = 2 ** (30 - CHILD), pieces: PieceBuilder[][] = [[], [], [], []];
    const x0 = owner.x * 2 * side, y0 = owner.y * 2 * side;
    const owns = ({ x, y }: Station) => x >= x0 && x < x0 + 2 * side && y >= y0 && y < y0 + 2 * side;
    /** The piece of the child that holds a station, with room for its faces. */
    const pieceAt = ({ x, y }: Station) => {
        const i = Math.floor((x - x0) / side), j = Math.floor((y - y0) / side), list = pieces[j * 2 + i];
        const last = list[list.length - 1];
        return last?.roomy ? last : list[list.push(new PieceBuilder(x0 + (i + 0.5) * side, y0 + (j + 0.5) * side)) - 1];
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
    }
    return { pieces: pieces.flat().map(piece => piece.piece(GROUND_M)) };
}

/** Bytes that a mesh holds. */
export const meshBytes = ({ pieces }: RoadMesh) => pieces.reduce((sum, { vertices, indices }) => sum + vertices.byteLength + indices.byteLength, 0);
