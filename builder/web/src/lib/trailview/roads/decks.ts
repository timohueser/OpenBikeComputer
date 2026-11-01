// The decks of road bridges, which corridors draw as ways of their junctions: the sidewalks that a deck takes from the
// road that runs on from it, and what gives way to a road that leaves its end. The stamp worker draws the deck's
// parapets and the road worker its surface, so both read the same plan, a pure function of the tile's lines.
import { BRIDGE, isPath, junctionWays, metresPerUnit, PATH, ROAD, walkOf, type Line } from '../terrain/bins';
import { LEFT, RIGHT } from '../terrain/town';
import { between, CUT, stationsOf, type Station } from './stations';
import type { Run } from './sweep';

/** A road's parapet: its width. */
export const PARAPET_M = 0.35;
/** A deck that leaves a road or a deck at a T starts at its edge, if its two ends together lose at most this share of its length. */
const TEE_SHARE = 0.5;
/** As `w.half < 2.0` of paved_look of looks.wgsl. */
const NARROW_M = 2;
/** A deck that runs on another deck's road within MEET_M of its height joins it there. */
const MEET_M = 1.0;
/** A road that leaves a deck's end counts with its way out this far, its core this far past its carriageway, and within SLACK_M past the deck's outer face. */
const LEAVES_M = 20, CORE_M = 0.6, SLACK_M = 0.5;
/** A side gives way over at most this share of the deck, and this far past the leaving road's core. */
const GIVE_SHARE = 0.45, GIVE_CLEAR_M = 0.3;

/** The half-width of a deck to the outer face of a road's parapets; a step past a path's railing still stands on it. */
export const deckHalf = (line: Line) => line.h + PARAPET_M;

/** A paved road or path, by its surface, as `paved` of roads.wgsl: its deck is painted, not planked. */
export const isPaved = (line: Line) => line.code <= 2 || line.code === 7;

/** A road bridge whose surface a corridor draws near the eye. A look that leaves pixels to the ground, of soil or of a narrow way with grass over its edges, would open holes in a deck: its paint stays. */
export const isDeck = (line: Line) => line.kind === BRIDGE && !isPath(line) && isPaved(line) && line.h >= NARROW_M;

/** A deck's plan. `walks`: its sidewalks' widths, left and right of its line. `yields`: at its start and at its end, the metres that its left and its right give to a road that leaves there. */
export interface DeckPlan { walks: [number, number]; yields: [[number, number], [number, number]] }

const plans = new WeakMap<Line[], Map<Line, DeckPlan>>();

/** The plans of a tile's decks, made once. */
export function decksOf(lines: Line[]): Map<Line, DeckPlan> {
    const known = plans.get(lines);
    if (known) return known;
    const ways = new Map(junctionWays(lines).map(list => [`${list[0].line.xy[2 * list[0].v]},${list[0].line.xy[2 * list[0].v + 1]}`, list] as const)), tees = teesOf(lines);
    const plan = new Map<Line, DeckPlan>(), stationsFor = new Map<Line, Station[]>();
    const stations = (line: Line) => stationsFor.get(line) ?? stationsFor.set(line, stationsOf(line, () => line.h)).get(line)!;
    for (const line of lines.filter(isDeck)) {
        // Only a beam bridge has the parapets that a walk lies behind and that give way.
        if (line.style !== 'beam') {
            plan.set(line, { walks: [0, 0], yields: [[0, 0], [0, 0]] });
            continue;
        }
        const list = stations(line), total = list[list.length - 1].along, cut = tees(line, total), walks: DeckPlan['walks'] = [0, 0], here = [0, line.z.length - 1].map(v => ways.get(`${line.xy[2 * v]},${line.xy[2 * v + 1]}`) ?? []);
        here.forEach((at, end) => {
            // The way that runs on from the deck is the road that it carries.
            for (const { line: road, v, step, side } of at) {
                if (side || road.kind !== ROAD || !road.sidewalks) continue;
                const flags = road.sidewalks[step > 0 ? v : v - 1] ?? 0, same = step < 0 === (end === 0), width = walkOf(road);
                if (flags & (same ? LEFT : RIGHT)) walks[0] = Math.max(walks[0], width);
                if (flags & (same ? RIGHT : LEFT)) walks[1] = Math.max(walks[1], width);
            }
        });
        const outer = [line.h + walks[0] + PARAPET_M, line.h + walks[1] + PARAPET_M], frame = frameOf(list);
        const yields = here.map((at, end): [number, number] => {
            const out: [number, number] = [0, 0];
            // Where the deck itself starts at a road's edge, nothing stands to give way.
            if (end ? cut[1] < total : cut[0] > 0) return out;
            for (const { line: road, v, step, side } of at) {
                if (!side || road.kind !== ROAD) continue;
                const along = stations(road), from = along.findIndex(s => s.x === road.xy[2 * v] && s.y === road.xy[2 * v + 1]), core = road.h + CORE_M;
                for (let k = from; from >= 0 && k >= 0 && k < along.length && Math.abs(along[k].along - along[from].along) <= LEAVES_M; k += step) {
                    for (const a of [-core, 0, core]) {
                        const [metres, left] = frame(along[k].x + along[k].ax * a, along[k].y + along[k].ay * a), d = end ? total - metres : metres;
                        if (d <= 0 || d > GIVE_SHARE * total) continue;
                        if (left > line.h && left <= outer[0] + SLACK_M) out[0] = Math.max(out[0], d);
                        if (-left > line.h && -left <= outer[1] + SLACK_M) out[1] = Math.max(out[1], d);
                    }
                }
            }
            return out.map(d => (d > 0 ? Math.min(GIVE_SHARE * total, d + GIVE_CLEAR_M) : 0)) as [number, number];
        }) as DeckPlan['yields'];
        plan.set(line, { walks, yields });
    }
    plans.set(lines, plan);
    return plan;
}

/** Marks what a deck's corridor leaves out: short of a T, on a side that gives way, and where a wider deck carries it. */
export function markDeck(run: Run, lines: Line[], mark: (way: { run: Run }, from: number, to: number, bits: number) => void): void {
    const { line, stations } = run, total = stations[stations.length - 1].along, [from, to] = teesOf(lines)(line, total), carriers = wider(line, lines), plan = decksOf(lines).get(line);
    if (from > 0) mark({ run }, 0, from, CUT.ALL);
    if (to < total) mark({ run }, total, to, CUT.ALL);
    plan?.yields.forEach((sides, end) => sides.forEach((d, i) => {
        if (d > 0) mark({ run }, end ? total : 0, end ? total - d : d, i ? CUT.RIGHT : CUT.LEFT);
    }));
    if (carriers.length) for (let j = 0; j + 1 < stations.length; j++) if (onDeck(between(stations[j], stations[j + 1], 0.5), carriers)) mark({ run }, stations[j].along, stations[j + 1].along, CUT.ALL);
}

/** A world point's metres along stations, and to their left. */
const frameOf = (list: Station[]) => (x: number, y: number): [number, number] => {
    let best: [number, number] = [0, 0], least = Infinity;
    for (let k = 0; k + 1 < list.length; k++) {
        const s = list[k], t = list[k + 1], unit = metresPerUnit(s.y), dx = (t.x - s.x) * unit, dy = (t.y - s.y) * unit, l2 = dx * dx + dy * dy || 1;
        const ex = (x - s.x) * unit, ey = (y - s.y) * unit, u = Math.max(0, Math.min(1, (ex * dx + ey * dy) / l2)), d = Math.hypot(ex - u * dx, ey - u * dy);
        if (d < least) [least, best] = [d, [s.along + u * (t.along - s.along), (ex * dy - ey * dx) / Math.sqrt(l2)]];
    }
    return best;
};

const pointKey = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;

/** Given a bridge line of `total` metres, the metres from its start where its deck starts and ends: short of an end that leaves a road or a deck at a T. */
export function teesOf(all: Line[]): (line: Line, total: number) => [number, number] {
    // The inner vertices of the roads on the ground and of the decks, with the half-width that a deck leaving them starts past.
    const through = new Map<string, number>();
    for (const line of all) {
        const half = line.kind === BRIDGE ? deckHalf(line) : line.kind === ROAD || line.kind === PATH ? line.h : 0;
        for (let v = 1; half && v + 1 < line.z.length; v++) through.set(pointKey(line, v), Math.max(through.get(pointKey(line, v)) ?? 0, half));
    }
    return (line, total) => {
        const cuts = [through.get(pointKey(line, 0)) ?? 0, through.get(pointKey(line, line.z.length - 1)) ?? 0];
        return cuts[0] + cuts[1] < TEE_SHARE * total ? [cuts[0], total - cuts[1]] : [0, total];
    };
}

/** The decks wider than `line` that it may run on: not those that it runs on into at a joint, and only those whose road comes within its box. */
export function wider(line: Line, lines: Line[]): Line[] {
    const joints = new Set([pointKey(line, 0), pointKey(line, line.z.length - 1)]), [x0, y0, x1, y1] = box(line.xy);
    const unit = Math.min(metresPerUnit(y0), metresPerUnit(y1));
    return lines.filter(other => {
        if (other.kind !== BRIDGE || other.h <= line.h || joints.has(pointKey(other, 0)) || joints.has(pointKey(other, other.z.length - 1))) return false;
        const [a0, b0, a1, b1] = box(other.xy), grow = other.h / unit;
        return a0 - grow <= x1 && a1 + grow >= x0 && b0 - grow <= y1 && b1 + grow >= y0;
    });
}

/** The box of a line's points: west, north, east, south in world units. */
function box(xy: ArrayLike<number>): [number, number, number, number] {
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (let i = 0; i < xy.length; i += 2) {
        x0 = Math.min(x0, xy[i]);
        x1 = Math.max(x1, xy[i]);
        y0 = Math.min(y0, xy[i + 1]);
        y1 = Math.max(y1, xy[i + 1]);
    }
    return [x0, y0, x1, y1];
}

/** Whether a point of a deck lies on the road of one of the wider `decks` within MEET_M of its height. */
export function onDeck(s: { x: number; y: number; z: number }, decks: Line[]): boolean {
    const unit = metresPerUnit(s.y);
    for (const { xy, z, h } of decks) {
        for (let v = 0; v + 1 < z.length; v++) {
            const ax = xy[2 * v], ay = xy[2 * v + 1], dx = xy[2 * v + 2] - ax, dy = xy[2 * v + 3] - ay;
            const u = Math.min(1, Math.max(0, ((s.x - ax) * dx + (s.y - ay) * dy) / (dx * dx + dy * dy || 1)));
            if (Math.hypot(s.x - ax - u * dx, s.y - ay - u * dy) * unit <= h && Math.abs(s.z - z[v] - (z[v + 1] - z[v]) * u) < MEET_M) return true;
        }
    }
    return false;
}
