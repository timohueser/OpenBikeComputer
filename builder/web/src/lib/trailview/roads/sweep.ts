// The faces of a corridor: each station's cross-section joined to the next one's, and a round cap at a
// dead end. A skirt's height is known only on the GPU, so faces are wound from their plan (seen from above,
// counter-clockwise), and a face that stands upright on a side faces the line's middle, as a kerb does.
import { metresPerUnit, RAIL, type Line } from '../terrain/bins';
import { lookWord, type PieceBuilder } from './piece';
import { isDeck, PARAPET_M } from './decks';
import type { Paint } from './marks';
import { carriageway, CF, drawnCode, junctionFace, lowered, sideOf, type Part } from './section';
import { across, CUT, facesOf, foldOf, type Station, strokeMetres } from './stations';
import { sideStop, type Shore } from './shore';
import type { Daylight } from './skirt';
import { onWay, type Ways } from './ways';

/** Steps of a dead end's half circle near the eye, and in the far range. */
const CAP_STEPS = 10, FAR_CAP_STEPS = 5;
/** Repeats along a way divide this count, so a base of whole multiples keeps their phase. */
const REPEAT = 720, OWNER_REPEATS = 2;

/**
 * A line's corridor: its stations, the reach of their sides, its height over the core and the base of its counts along.
 * Where water lies near: its shore, and per station the metres past the carriageway's edge where its left and its right side stop.
 * Where the ground is known: the daylight of its skirt; without it, each skirt is that of flat ground.
 */
export interface Run { id: number; line: Line; stations: Station[]; reach: Float64Array; lift: number; base: number; ways: Ways; shore?: Shore; stops?: Float64Array; daylight?: Daylight }

/** A side's points without a skirt's row that repeats the one before it: a cap or an end has no partner station to pair with. */
const distinct = (parts: Part[]) => parts.filter((part, i) => i === 0 || part.a !== parts[i - 1].a || part.flags !== parts[i - 1].flags);

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

/** A skirt's row: a backfill's row holds both of these flags, and stands at its own height. */
function skirtRow(flags: number): boolean {
    const row = flags & CF.FILL;
    return row === CF.SAMPLE || row === CF.MESH;
}

/** A vertex's flags: a skirt's row over another way's carriageway sinks under it. */
export function under(ways: Ways, flags: number, x: number, y: number): number {
    return skirtRow(flags) && onWay(ways, x, y) ? flags | CF.UNDER : flags;
}

/** The flag of a line's faces that stand over the ground: a deck's. */
const deckFlag = (line: Line) => (isDeck(line) ? CF.DECK : 0);

/**
 * The vertex of point `part` of a cross-section at station j, `a` metres across, in signed metres. A skirt's
 * vertex holds the road's plane in z, so the shader can tell a cut, and the core's outer edge `edge` for its half-width.
 */
export function point(b: PieceBuilder, run: Run, j: number, part: Part, a: number, edge: number): number {
    const key = run.id * 2 ** 21 + j, slots = b.made.get(key) ?? b.made.set(key, []).get(key)!;
    for (const slot of slots) if (slot.a === a && slot.dz === part.dz && slot.flags === part.flags) return slot.index;
    const s = run.stations[j], [x, y, metres] = across(s, a), plane = s.z + s.bank * metres + run.lift, skirt = skirtRow(part.flags);
    const index = b.vertex(x, y, skirt ? plane : plane + part.dz, lookWord(part.mat, drawnCode(run.line), under(run.ways, part.flags, x, y) | deckFlag(run.line), skirt ? edge : s.h), a, countOf(run, s));
    slots.push({ a, dz: part.dz, flags: part.flags, index });
    return index;
}

/** The vertex of point `part` of paint at station j: faces of one paint share it, and no others. */
function painted(b: PieceBuilder, run: Run, j: number, part: Part, paint: Paint): number {
    const key = run.id * 2 ** 21 + j, slots = b.made.get(key) ?? b.made.set(key, []).get(key)!, flags = -paint.mat;
    for (const slot of slots) if (slot.a === part.a && slot.flags === flags) return slot.index;
    const s = run.stations[j], [x, y, metres] = across(s, part.a), z = s.z + s.bank * metres + run.lift + part.dz;
    const index = b.vertex(x, y, z, lookWord(paint.mat, drawnCode(run.line), deckFlag(run.line), s.h), paint.across(s, x, y, part.a), paint.along(s, countOf(run, s)));
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

/**
 * The carriageways of two stations held to a step's fold, metres across on its left and its right: the points past it give way
 * to one on it at the facet's height there. Inside a bend tighter than the carriageway the asphalt then ends where its offset
 * shrinks to nothing, and no face of it turns over. Both keep one count of points. Two steps that meet at a station may hold
 * it at two folds: the point between them then lies on the other step's edge, with no vertex there.
 */
function folded(road: Part[], next: Part[], [left, right]: [number, number]): [Part[], Part[]] {
    const on = (p: Part, q: Part, a: number): Part => ({ ...p, a, dz: p.dz + (q.dz - p.dz) * (a - p.a) / (q.a - p.a) });
    let first = 0, end = road.length - 1;
    while (first + 1 < end && Math.max(road[first + 1].a, next[first + 1].a) >= left) first++;
    while (end - 1 > first && Math.min(road[end - 1].a, next[end - 1].a) <= -right) end--;
    const held = (parts: Part[]) => {
        const out = parts.slice(first, end + 1);
        if (out[0].a > left) out[0] = on(out[1], out[0], left);
        if (out.at(-1)!.a < -right) out[out.length - 1] = on(out.at(-2)!, out.at(-1)!, -right);
        return out;
    };
    return first === 0 && end === road.length - 1 && road[0].a <= left && next[0].a <= left && -road[end].a <= right && -next[end].a <= right ? [road, next] : [held(road), held(next)];
}

/**
 * The carriageway's points at station j, left to right. On the inner side of a bend, the strip between two cross-sections
 * is narrower at the edge than in the middle, so the slope along it grows toward the edge by that ratio, and one flat face
 * across it would shade as a wedge. Rows split that half into faces (`facesOf`) at
 * distances from where the cross-sections meet (the bend's centre, or a fan's pivot) in a geometric series. A level way
 * gets no row, and neither does a bend tighter than its carriageway, which folds.
 */
function section(run: Run, j: number, parts: Part[]): Part[] {
    const { stations } = run, s = stations[j], before = stations[j - 1], after = stations[j + 1];
    if (!before || !after) return parts;
    // Where the cross-sections meet: the pivot of a fan that holds the station or its neighbour, else the bend's centre.
    const rung = s.rung ?? before.rung ?? after.rung, at = s.rung ? s : before.rung ? before : after;
    const [left, right] = foldOf(stations, j - 1, j + 1), side = rung?.side ?? (left < right ? 1 : -1), bend = Math.min(left, right);
    let [middle, edge] = [bend, bend - s.h];
    if (rung) {
        const m = metresPerUnit(s.y), qx = at.x + rung.ux * (rung.length + rung.r), qy = at.y + rung.uy * (rung.length + rung.r), [x, y] = across(s, side * s.h);
        [middle, edge] = [Math.hypot(qx - s.x, qy - s.y) * m, Math.hypot(qx - x, qy - y) * m];
    }
    if (!(edge >= 0.5 && middle - edge > 0.05 && middle < 1e4)) return parts;
    const grade = Math.abs(after.z - before.z) / Math.max(after.along - before.along, 1e-3);
    const faces = facesOf(run.line, grade, middle, edge), out = [...parts];
    for (let k = 1; k < faces; k++) {
        const a = side * s.h * (middle - edge * (middle / edge) ** (k / faces)) / (middle - edge);
        const i = out.findIndex((part, c) => c + 1 < out.length && part.a >= a && out[c + 1].a <= a);
        if (i < 0 || Math.min(Math.abs(out[i].a - a), Math.abs(out[i + 1].a - a)) < 0.05) continue;
        const [p, q] = [out[i], out[i + 1]];
        out.splice(i + 1, 0, { ...p, a, dz: p.dz + (q.dz - p.dz) * (a - p.a) / (q.a - p.a), far: false });
    }
    return out;
}

/**
 * The faces between the points of two cross-sections, left to right, `shares` of each across its half-width. With the
 * same count of points, the quads of a strip. Else a zipper steps along the one whose next point lies first, so that the
 * rows of either fit. Each face's first vertex lies at the first station, wound as `quad`.
 */
function zip(list: number[], here: number[], shares: number[], there: number[], others: number[]): void {
    if (here.length === there.length) {
        for (let c = 0; c + 1 < here.length; c++) quad(list, here[c], here[c + 1], there[c + 1], there[c], false);
        return;
    }
    for (let i = 0, l = 0; i + 1 < here.length || l + 1 < there.length;) {
        if (l + 1 >= there.length || (i + 1 < here.length && shares[i + 1] >= others[l + 1])) list.push(here[i + 1], there[l], here[i++]);
        else list.push(here[i], there[l + 1], there[l++]);
    }
}

/** The faces between stations j and k into `list`: the carriageway, then each side out to the end of its skirt. `far` keeps the points of the far range. */
export function sweep(b: PieceBuilder, list: number[], run: Run, j: number, k: number, far: boolean): void {
    const { line, stations, reach } = run, s = stations[j], t = stations[k], cut = s.cut ?? 0;
    // A face's flags are on its first vertex, at the first station.
    const flags = (cut & CUT.LEFT ? CF.NO_LEFT_LINE : 0) | (cut & CUT.RIGHT ? CF.NO_RIGHT_LINE : 0);
    const roads = [section(run, j, carriageway(line, s.h, s.bank)).map(part => ({ ...part, flags: part.flags | flags })), section(run, k, carriageway(line, t.h, t.bank))].map(road => (far ? road.filter(part => part.far) : road));
    // A bend whose carriageway folds has no rows.
    const [road, next] = roads[0].length === roads[1].length ? folded(roads[0], roads[1], foldOf(stations, j, k)) : roads;
    if (!(cut & CUT.ROAD)) {
        const at = (index: number, part: Part) => (s.paint ? painted(b, run, index, part, s.paint) : point(b, run, index, part, part.a, 0));
        zip(list, road.map(part => at(j, part)), road.map(({ a }) => a / s.h), next.map(part => at(k, part)), next.map(({ a }) => a / t.h));
    }
    for (const side of [1, -1] as const) {
        const edgeS = road[side > 0 ? 0 : road.length - 1], edgeT = next[side > 0 ? 0 : next.length - 1];
        if (cut & (side > 0 ? CUT.LEFT : CUT.RIGHT)) {
            if (isDeck(line) && !(cut & CUT.ROAD)) apron(b, list, run, j, k, side, edgeS.dz);
            continue;
        }
        const most = side > 0 ? 0 : 1, fanned = s.rung?.side === side || t.rung?.side === side, here = sideOf(line, side, t.v, s.h, run.daylight?.(j, most), fanned), there = sideOf(line, side, t.v, t.h, run.daylight?.(k, most), fanned), which = most;
        const parts = lowered(here.parts, far ? 0 : s.drop?.[which] ?? 0, far ? 0 : s.end?.[which] ?? 0), others = lowered(there.parts, far ? 0 : t.drop?.[which] ?? 0, far ? 0 : t.end?.[which] ?? 0);
        const reachS = reach[2 * j + most], reachT = reach[2 * k + most];
        let ps = point(b, run, j, edgeS, edgeS.a, 0), pt = point(b, run, k, edgeT, edgeT.a, 0);
        for (let c = 0; c < parts.length && parts[c].a <= reachS && others[c].a <= reachT; c++) {
            if (far && !parts[c].far) continue;
            const qs = point(b, run, j, parts[c], side * ashore(run, j, most, parts[c].a), here.edge), qt = point(b, run, k, others[c], side * ashore(run, k, most, others[c].a), there.edge);
            // Two rows that fall on one another at both stations leave no face.
            if (qs !== ps || qt !== pt) quad(list, ps, qs, qt, pt, side > 0);
            ps = qs;
            pt = qt;
        }
    }
}

/** A deck's top from its carriageway's edge at height `dz` to its outer face, on a side that gives way to a road: one surface with that road's mouth. */
function apron(b: PieceBuilder, list: number[], run: Run, j: number, k: number, side: 1 | -1, dz: number): void {
    const { line, stations, lift } = run, { mat, half, across } = junctionFace(line)!, word = lookWord(mat, drawnCode(line), CF.DECK, half);
    const edge = (s: Station, a: number) => b.vertex(s.x + s.ax * a, s.y + s.ay * a, s.z + s.bank * a + lift + dz, word, across, 0);
    const [s, t] = [stations[j], stations[k]], outer = (at: Station) => side * (sideOf(line, side, at.v, at.h).edge + PARAPET_M);
    quad(list, edge(s, side * s.h), edge(s, outer(s)), edge(t, outer(t)), edge(t, side * t.h), side > 0);
}

/** A dead end's half circle at station j, ahead `dir` along the line: the carriageway's end as a fan, then the skirt's rings, out to the farther daylight of its sides, which hold their radial metres. */
export function cap(b: PieceBuilder, list: number[], run: Run, j: number, dir: 1 | -1, far: boolean): void {
    const { line, stations } = run, s = stations[j], steps = far ? FAR_CAP_STEPS : CAP_STEPS, road = carriageway(line, s.h, 0), middle = road[(road.length - 1) / 2];
    const lit = run.daylight && Math.max(run.daylight(j, 0), run.daylight(j, 1)), { parts, edge } = sideOf(line, 1, s.v, s.h, lit), fx = -s.ay * dir, fy = s.ax * dir, count = countOf(run, s);
    const at = (to: number, step: number, part: Part) => {
        const angle = Math.PI * step / steps, cos = Math.cos(angle), sin = Math.sin(angle), dx = s.ax * cos + fx * sin, dy = s.ay * cos + fy * sin;
        const r = run.shore ? Math.min(to, s.h + sideStop(run.shore, s.x + dx * s.h, s.y + dy * s.h, dx, dy, () => edge - s.h)) : to, x = s.x + dx * r, y = s.y + dy * r;
        const skirt = (part.flags & (CF.SAMPLE | CF.MESH)) !== 0;
        return b.vertex(x, y, s.z + run.lift + (skirt ? 0 : part.dz), lookWord(part.mat, drawnCode(line), under(run.ways, part.flags, x, y), skirt ? edge : s.h), r, count);
    };
    const centre = b.vertex(s.x, s.y, s.z + run.lift + middle.dz, lookWord(middle.mat, drawnCode(line), 0, s.h), 0, count);
    const ahead = dir > 0;
    let inner = Array.from({ length: steps + 1 }, (_, step) => at(s.h, step, road[0]));
    for (let step = 0; step < steps; step++) list.push(...(ahead ? [centre, inner[step + 1], inner[step]] : [centre, inner[step], inner[step + 1]]));
    for (const part of distinct(far ? parts.filter(({ far: kept }) => kept) : parts)) {
        const outer = Array.from({ length: steps + 1 }, (_, step) => at(part.a, step, part));
        for (let step = 0; step < steps; step++) quad(list, inner[step], outer[step], outer[step + 1], inner[step + 1], ahead);
        inner = outer;
    }
}

/** A plain end at station j, ahead `dir` along the line, where a lesser way runs on: the section's points, the skirt's included, each joined to the ground ahead over the left skirt's rows. No kerb or sidewalk turns across the way. */
export function plainEnd(b: PieceBuilder, list: number[], run: Run, j: number, dir: 1 | -1, far: boolean): void {
    const { line, stations } = run, s = stations[j], kept = (parts: Part[]) => distinct(far ? parts.filter(part => part.far) : parts), fx = -s.ay * dir, fy = s.ax * dir, count = countOf(run, s);
    const [left, right] = [sideOf(line, 1, s.v, s.h, run.daylight?.(j, 0)), sideOf(line, -1, s.v, s.h, run.daylight?.(j, 1))], road = kept(carriageway(line, s.h, s.bank));
    const top = [...kept(left.parts).reverse().map(part => point(b, run, j, part, part.a, left.edge)), ...road.map(part => point(b, run, j, part, part.a, 0)), ...kept(right.parts).map(part => point(b, run, j, part, -part.a, right.edge))];
    const across = [...kept(left.parts).reverse().map(({ a }) => a), ...road.map(({ a }) => a), ...kept(right.parts).map(({ a }) => -a)], rows = kept(left.parts).filter(({ flags }) => flags & CF.FILL);
    let inner = top;
    for (const row of rows) {
        const metres = row.a - left.edge, outer = across.map(a => {
            const x = s.x + s.ax * a + fx * metres, y = s.y + s.ay * a + fy * metres;
            return b.vertex(x, y, s.z + s.bank * a + run.lift, lookWord(row.mat, drawnCode(line), under(run.ways, row.flags, x, y), left.edge), left.edge + metres, count);
        });
        for (let k = 0; k + 1 < top.length; k++) quad(list, inner[k], outer[k], outer[k + 1], inner[k + 1], dir > 0);
        inner = outer;
    }
}

/** A carriageway's lift over the stamped core: the better class higher, so two ways at a junction keep apart. */
export const liftOf = (line: Line, id: number) => Math.max(0, 30 - line.profile) * 0.001 + (id % 2) * 0.0005;
