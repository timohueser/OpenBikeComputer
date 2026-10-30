// The faces of a corridor: each station's cross-section joined to the next one's, and a round cap at a
// dead end. A band's height is known only on the GPU, so faces are wound from their plan (seen from above,
// counter-clockwise), and a face that stands upright on a side faces the line's middle, as a kerb does.
import { RAIL, type Line } from '../terrain/bins';
import { lookWord, type PieceBuilder } from './piece';
import { carriageway, CF, sideOf, type Part } from './section';
import type { Station } from './stations';
import { onWay, type Ways } from './ways';

/** Steps of a dead end's half circle near the eye, and in the far range. */
const CAP_STEPS = 10, FAR_CAP_STEPS = 5;
/** Repeats along a way divide this count, so a base of whole multiples keeps their phase. */
const REPEAT = 720;

/** A line's corridor: its stations, the reach of their sides, its height over the core, the band's priority and the base of its counts along. */
export interface Run { id: number; line: Line; stations: Station[]; reach: Float64Array; lift: number; prio: number; base: number; ways: Ways }

/** What a station counts along its line, from the base of the run: sleepers on a track, else metres. */
const countOf = ({ line, base }: Run, s: Station) => (line.kind === RAIL ? s.sleepers ?? 0 : s.along) - base;

/** The base of a run from its first station in an owner. */
export const baseOf = (line: Line, s: Station) => Math.floor((line.kind === RAIL ? s.sleepers ?? 0 : s.along) / REPEAT) * REPEAT;

/**
 * The vertex of point `part` of a cross-section at station j, `a` metres across, in signed metres. A band's
 * vertex holds the road's plane in z, so the shader can tell a cut, and the core's outer edge `edge` for its half-width.
 */
function point(b: PieceBuilder, run: Run, j: number, part: Part, a: number, edge: number): number {
    const key = run.id * 2 ** 21 + j, slots = b.made.get(key) ?? b.made.set(key, []).get(key)!;
    for (const slot of slots) if (slot.a === a && slot.dz === part.dz && slot.flags === part.flags) return slot.index;
    const s = run.stations[j], plane = s.z + s.bank * a + run.lift, x = s.x + s.ax * a, y = s.y + s.ay * a, band = (part.flags & (CF.SAMPLE | CF.MESH)) !== 0;
    const flags = band && onWay(run.ways, x, y) ? part.flags | CF.UNDER : part.flags;
    const index = b.vertex(x, y, band ? plane : plane + part.dz, lookWord(part.mat, run.line.code, flags, run.prio, band ? edge : s.h), a, countOf(run, s));
    slots.push({ a, dz: part.dz, flags: part.flags, index });
    return index;
}

/** The two triangles of a quad, each with its first vertex on q at the first station. */
function quad(list: number[], ps: number, qs: number, qt: number, pt: number, left: boolean): void {
    if (left) list.push(qs, pt, qt, qs, ps, pt);
    else list.push(qs, qt, pt, qs, pt, ps);
}

/** The faces between stations j and k into `list`: the carriageway, then each side out to the end of its band. `far` keeps the points of the far range. */
export function sweep(b: PieceBuilder, list: number[], run: Run, j: number, k: number, far: boolean): void {
    const { line, stations, reach } = run, s = stations[j], t = stations[k];
    const roads = [carriageway(line, s.h, s.bank), carriageway(line, t.h, t.bank)].map(road => (far ? road.filter(part => part.far) : road));
    const [road, next] = roads, last = road.length - 1;
    for (let c = 0; c < last; c++) {
        const ps = point(b, run, j, road[c], road[c].a, 0), qs = point(b, run, j, road[c + 1], road[c + 1].a, 0), qt = point(b, run, k, next[c + 1], next[c + 1].a, 0), pt = point(b, run, k, next[c], next[c].a, 0);
        quad(list, ps, qs, qt, pt, false);
    }
    for (const side of [1, -1] as const) {
        const outer = side > 0 ? 0 : last, here = sideOf(line, side, t.v, s.h), there = sideOf(line, side, t.v, t.h), parts = here.parts, others = there.parts;
        const most = side > 0 ? 0 : 1, reachS = reach[2 * j + most], reachT = reach[2 * k + most];
        let ps = point(b, run, j, road[outer], road[outer].a, 0), pt = point(b, run, k, next[outer], next[outer].a, 0);
        for (let c = 0; c < parts.length && parts[c].a <= reachS && others[c].a <= reachT; c++) {
            if (far && !parts[c].far) continue;
            const qs = point(b, run, j, parts[c], side * parts[c].a, here.edge), qt = point(b, run, k, others[c], side * others[c].a, there.edge);
            quad(list, ps, qs, qt, pt, side > 0);
            ps = qs;
            pt = qt;
        }
    }
}

/** A dead end's half circle at station j, ahead `dir` along the line: the carriageway's end as a fan, then the band's rings, which hold their radial metres. */
export function cap(b: PieceBuilder, list: number[], run: Run, j: number, dir: 1 | -1, far: boolean): void {
    const { line, stations } = run, s = stations[j], steps = far ? FAR_CAP_STEPS : CAP_STEPS, road = carriageway(line, s.h, 0), middle = road[(road.length - 1) / 2];
    const { parts, edge } = sideOf(line, 1, s.v, s.h), fx = -s.ay * dir, fy = s.ax * dir, count = countOf(run, s);
    const at = (r: number, step: number, part: Part) => {
        const angle = Math.PI * step / steps, cos = Math.cos(angle), sin = Math.sin(angle), x = s.x + (s.ax * cos + fx * sin) * r, y = s.y + (s.ay * cos + fy * sin) * r;
        const band = (part.flags & (CF.SAMPLE | CF.MESH)) !== 0, flags = band && onWay(run.ways, x, y) ? part.flags | CF.UNDER : part.flags;
        return b.vertex(x, y, s.z + run.lift + (band ? 0 : part.dz), lookWord(part.mat, line.code, flags, run.prio, band ? edge : s.h), r, count);
    };
    const centre = b.vertex(s.x, s.y, s.z + run.lift + middle.dz, lookWord(middle.mat, line.code, 0, run.prio, s.h), 0, count);
    const ahead = dir > 0;
    let inner = Array.from({ length: steps + 1 }, (_, step) => at(s.h, step, road[0]));
    for (let step = 0; step < steps; step++) list.push(...(ahead ? [centre, inner[step + 1], inner[step]] : [centre, inner[step], inner[step + 1]]));
    for (const part of far ? parts.filter(({ far: kept }) => kept) : parts) {
        const outer = Array.from({ length: steps + 1 }, (_, step) => at(part.a, step, part));
        for (let step = 0; step < steps; step++) quad(list, inner[step], outer[step], outer[step + 1], inner[step + 1], ahead);
        inner = outer;
    }
}

/** A carriageway's lift over the stamped core: the better class higher, so two ways at a junction keep apart. */
export const liftOf = (line: Line, id: number) => Math.max(0, 30 - line.profile) * 0.001 + (id % 2) * 0.0005;
/** The band of the better class draws over the other where two bands overlap. */
export const prioOf = (line: Line) => Math.max(0, Math.min(7, 7 - (line.profile >> 2)));
