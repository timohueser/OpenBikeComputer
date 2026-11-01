// The faces of a corridor: each station's cross-section joined to the next one's, and a round cap at a
// dead end. A band's height is known only on the GPU, so faces are wound from their plan (seen from above,
// counter-clockwise), and a face that stands upright on a side faces the line's middle, as a kerb does.
import { RAIL, type Line } from '../terrain/bins';
import { lookWord, type PieceBuilder } from './piece';
import { isDeck, PARAPET_M } from './decks';
import type { Paint } from './marks';
import { carriageway, CF, drawnCode, junctionFace, lowered, sideOf, type Part } from './section';
import { CUT, type Station, strokeMetres } from './stations';
import { sideStop, type Shore } from './shore';
import { onWay, type Ways } from './ways';

/** Steps of a dead end's half circle near the eye, and in the far range. */
const CAP_STEPS = 10, FAR_CAP_STEPS = 5;
/** Repeats along a way divide this count, so a base of whole multiples keeps their phase. */
const REPEAT = 720, OWNER_REPEATS = 2;

/**
 * A line's corridor: its stations, the reach of their sides, its height over the core, the band's priority and the base of its counts along.
 * Where water lies near: its shore, and per station the metres past the carriageway's edge where its left and its right side stop.
 */
export interface Run { id: number; line: Line; stations: Station[]; reach: Float64Array; lift: number; prio: number; base: number; ways: Ways; shore?: Shore; stops?: Float64Array }

/** Per station, the metres past the carriageway's edge where each side of a line stops at water: left, then right. */
export function stopsOf(line: Line, stations: Station[], shore: Shore): Float64Array {
    const stops = new Float64Array(2 * stations.length);
    stations.forEach((s, j) => {
        for (const side of [1, -1] as const) stops[2 * j + (side > 0 ? 0 : 1)] = sideStop(shore, s.x + s.ax * side * s.h, s.y + s.ay * side * s.h, side * s.ax, side * s.ay, () => sideOf(line, side, s.v, s.h).edge - s.h);
    });
    return stops;
}

/** What a station counts along its line: sleepers on a track, metres along its stroke on a road, else metres. */
const counted = (line: Line, s: Station) => (line.kind === RAIL ? s.sleepers ?? 0 : strokeMetres(line, s.along));

/** What a station counts from the base of the run. */
export const countOf = ({ line, base }: Run, s: Station) => counted(line, s) - base;

/** The base of a run from its first station in an owner. Counts fall along a line that runs against its stroke, by less than OWNER_REPEATS repeats within an owner. */
export const baseOf = (line: Line, s: Station) => (Math.floor(counted(line, s) / REPEAT) - OWNER_REPEATS) * REPEAT;

/** A band's row: a backfill's row holds both of these flags, and stands at its own height. */
function bandRow(flags: number): boolean {
    const row = flags & CF.FILL;
    return row === CF.SAMPLE || row === CF.MESH;
}

/** A vertex's flags: a band's row over another way's carriageway sinks under it. */
export function under(ways: Ways, flags: number, x: number, y: number): number {
    return bandRow(flags) && onWay(ways, x, y) ? flags | CF.UNDER : flags;
}

/**
 * The vertex of point `part` of a cross-section at station j, `a` metres across, in signed metres. A band's
 * vertex holds the road's plane in z, so the shader can tell a cut, and the core's outer edge `edge` for its half-width.
 */
export function point(b: PieceBuilder, run: Run, j: number, part: Part, a: number, edge: number): number {
    const key = run.id * 2 ** 21 + j, slots = b.made.get(key) ?? b.made.set(key, []).get(key)!;
    for (const slot of slots) if (slot.a === a && slot.dz === part.dz && slot.flags === part.flags) return slot.index;
    const s = run.stations[j], plane = s.z + s.bank * a + run.lift, x = s.x + s.ax * a, y = s.y + s.ay * a, band = bandRow(part.flags);
    const index = b.vertex(x, y, band ? plane : plane + part.dz, lookWord(part.mat, drawnCode(run.line), under(run.ways, part.flags, x, y), run.prio, band ? edge : s.h), a, countOf(run, s));
    slots.push({ a, dz: part.dz, flags: part.flags, index });
    return index;
}

/** The vertex of point `part` of paint at station j: faces of one paint share it, and no others. */
function painted(b: PieceBuilder, run: Run, j: number, part: Part, paint: Paint): number {
    const key = run.id * 2 ** 21 + j, slots = b.made.get(key) ?? b.made.set(key, []).get(key)!, flags = -paint.mat;
    for (const slot of slots) if (slot.a === part.a && slot.flags === flags) return slot.index;
    const s = run.stations[j], x = s.x + s.ax * part.a, y = s.y + s.ay * part.a, z = s.z + s.bank * part.a + run.lift + part.dz;
    const index = b.vertex(x, y, z, lookWord(paint.mat, drawnCode(run.line), 0, run.prio, s.h), paint.across(s, x, y, part.a), paint.along(s, countOf(run, s)));
    slots.push({ a: part.a, dz: part.dz, flags, index });
    return index;
}

/** The two triangles of a quad, each with its first vertex on q at the first station. */
export function quad(list: number[], ps: number, qs: number, qt: number, pt: number, left: boolean): void {
    if (left) list.push(qs, pt, qt, qs, ps, pt);
    else list.push(qs, qt, pt, qs, pt, ps);
}

/** `a` metres across a side of station j, held to the water's edge: the rows past it stand there, and the outer one still ends under the mesh. */
const ashore = ({ stations, stops }: Run, j: number, most: number, a: number) => (stops ? Math.min(a, stations[j].h + stops[2 * j + most]) : a);

/** The faces between stations j and k into `list`: the carriageway, then each side out to the end of its band. `far` keeps the points of the far range. */
export function sweep(b: PieceBuilder, list: number[], run: Run, j: number, k: number, far: boolean): void {
    const { line, stations, reach } = run, s = stations[j], t = stations[k], cut = s.cut ?? 0;
    // A face's flags are on its first vertex, at the first station.
    const flags = (cut & CUT.LEFT ? CF.NO_LEFT_LINE : 0) | (cut & CUT.RIGHT ? CF.NO_RIGHT_LINE : 0);
    const roads = [carriageway(line, s.h, s.bank).map(part => ({ ...part, flags })), carriageway(line, t.h, t.bank)].map(road => (far ? road.filter(part => part.far) : road));
    const [road, next] = roads, last = road.length - 1;
    for (let c = 0; c < last && !(cut & CUT.ROAD); c++) {
        const at = (index: number, part: Part) => (s.paint ? painted(b, run, index, part, s.paint) : point(b, run, index, part, part.a, 0));
        const ps = at(j, road[c]), qs = at(j, road[c + 1]), qt = at(k, next[c + 1]), pt = at(k, next[c]);
        quad(list, ps, qs, qt, pt, false);
    }
    for (const side of [1, -1] as const) {
        if (cut & (side > 0 ? CUT.LEFT : CUT.RIGHT)) {
            if (isDeck(line) && !(cut & CUT.ROAD)) apron(b, list, run, j, k, side, road[side > 0 ? 0 : last].dz);
            continue;
        }
        const outer = side > 0 ? 0 : last, here = sideOf(line, side, t.v, s.h), there = sideOf(line, side, t.v, t.h), which = side > 0 ? 0 : 1;
        const parts = lowered(here.parts, far ? 0 : s.drop?.[which] ?? 0), others = lowered(there.parts, far ? 0 : t.drop?.[which] ?? 0);
        const most = side > 0 ? 0 : 1, reachS = reach[2 * j + most], reachT = reach[2 * k + most];
        let ps = point(b, run, j, road[outer], road[outer].a, 0), pt = point(b, run, k, next[outer], next[outer].a, 0);
        for (let c = 0; c < parts.length && parts[c].a <= reachS && others[c].a <= reachT; c++) {
            if (far && !parts[c].far) continue;
            const qs = point(b, run, j, parts[c], side * ashore(run, j, most, parts[c].a), here.edge), qt = point(b, run, k, others[c], side * ashore(run, k, most, others[c].a), there.edge);
            quad(list, ps, qs, qt, pt, side > 0);
            ps = qs;
            pt = qt;
        }
    }
}

/** A deck's top from its carriageway's edge at height `dz` to its outer face, on a side that gives way to a road: one surface with that road's mouth. */
function apron(b: PieceBuilder, list: number[], run: Run, j: number, k: number, side: 1 | -1, dz: number): void {
    const { line, stations, prio, lift } = run, { mat, half, across } = junctionFace(line)!, word = lookWord(mat, drawnCode(line), 0, prio, half);
    const edge = (s: Station, a: number) => b.vertex(s.x + s.ax * a, s.y + s.ay * a, s.z + s.bank * a + lift + dz, word, across, 0);
    const [s, t] = [stations[j], stations[k]], outer = (at: Station) => side * (sideOf(line, side, at.v, at.h).edge + PARAPET_M);
    quad(list, edge(s, side * s.h), edge(s, outer(s)), edge(t, outer(t)), edge(t, side * t.h), side > 0);
}

/** A dead end's half circle at station j, ahead `dir` along the line: the carriageway's end as a fan, then the band's rings, which hold their radial metres. */
export function cap(b: PieceBuilder, list: number[], run: Run, j: number, dir: 1 | -1, far: boolean): void {
    const { line, stations } = run, s = stations[j], steps = far ? FAR_CAP_STEPS : CAP_STEPS, road = carriageway(line, s.h, 0), middle = road[(road.length - 1) / 2];
    const { parts, edge } = sideOf(line, 1, s.v, s.h), fx = -s.ay * dir, fy = s.ax * dir, count = countOf(run, s);
    const at = (to: number, step: number, part: Part) => {
        const angle = Math.PI * step / steps, cos = Math.cos(angle), sin = Math.sin(angle), dx = s.ax * cos + fx * sin, dy = s.ay * cos + fy * sin;
        const r = run.shore ? Math.min(to, s.h + sideStop(run.shore, s.x + dx * s.h, s.y + dy * s.h, dx, dy, () => edge - s.h)) : to, x = s.x + dx * r, y = s.y + dy * r;
        const band = (part.flags & (CF.SAMPLE | CF.MESH)) !== 0;
        return b.vertex(x, y, s.z + run.lift + (band ? 0 : part.dz), lookWord(part.mat, drawnCode(line), under(run.ways, part.flags, x, y), run.prio, band ? edge : s.h), r, count);
    };
    const centre = b.vertex(s.x, s.y, s.z + run.lift + middle.dz, lookWord(middle.mat, drawnCode(line), 0, run.prio, s.h), 0, count);
    const ahead = dir > 0;
    let inner = Array.from({ length: steps + 1 }, (_, step) => at(s.h, step, road[0]));
    for (let step = 0; step < steps; step++) list.push(...(ahead ? [centre, inner[step + 1], inner[step]] : [centre, inner[step], inner[step + 1]]));
    for (const part of far ? parts.filter(({ far: kept }) => kept) : parts) {
        const outer = Array.from({ length: steps + 1 }, (_, step) => at(part.a, step, part));
        for (let step = 0; step < steps; step++) quad(list, inner[step], outer[step], outer[step + 1], inner[step + 1], ahead);
        inner = outer;
    }
}

/** A plain end at station j, ahead `dir` along the line, where a lesser way runs on: the section's points, the band's included, each joined to the ground ahead over the band's rows. No kerb or sidewalk turns across the way. */
export function plainEnd(b: PieceBuilder, list: number[], run: Run, j: number, dir: 1 | -1, far: boolean): void {
    const { line, stations } = run, s = stations[j], kept = (parts: Part[]) => (far ? parts.filter(part => part.far) : parts), fx = -s.ay * dir, fy = s.ax * dir, count = countOf(run, s);
    const [left, right] = [sideOf(line, 1, s.v, s.h), sideOf(line, -1, s.v, s.h)], road = kept(carriageway(line, s.h, s.bank));
    const top = [...kept(left.parts).reverse().map(part => point(b, run, j, part, part.a, left.edge)), ...road.map(part => point(b, run, j, part, part.a, 0)), ...kept(right.parts).map(part => point(b, run, j, part, -part.a, right.edge))];
    const across = [...kept(left.parts).reverse().map(({ a }) => a), ...road.map(({ a }) => a), ...kept(right.parts).map(({ a }) => -a)], rows = kept(left.parts).filter(({ flags }) => flags & CF.FILL);
    let inner = top;
    for (const row of rows) {
        const metres = row.a - left.edge, outer = across.map(a => {
            const x = s.x + s.ax * a + fx * metres, y = s.y + s.ay * a + fy * metres;
            return b.vertex(x, y, s.z + s.bank * a + run.lift, lookWord(row.mat, drawnCode(line), under(run.ways, row.flags, x, y), run.prio, left.edge), left.edge + metres, count);
        });
        for (let k = 0; k + 1 < top.length; k++) quad(list, inner[k], outer[k], outer[k + 1], inner[k + 1], dir > 0);
        inner = outer;
    }
}

/** A carriageway's lift over the stamped core: the better class higher, so two ways at a junction keep apart. */
export const liftOf = (line: Line, id: number) => Math.max(0, 30 - line.profile) * 0.001 + (id % 2) * 0.0005;
/** The band of the better class draws over the other where two bands overlap. */
export const prioOf = (line: Line) => Math.max(0, Math.min(7, 7 - (line.profile >> 2)));
