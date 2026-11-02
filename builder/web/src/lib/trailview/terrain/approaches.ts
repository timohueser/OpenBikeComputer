// The heights of a lesser way where it leaves a road. The bake holds a way's heights from its junction on the road's
// middle, so at the road's outer edge a path or a track can stand metres off that edge. The stamp and the corridors
// both read a line's heights, so setting them here keeps the ground under the road's faces and the way on its ground.
import { pointOn, spanOf } from '../stream/curve';
import { SIDE_BACK, SIDE_ON } from '../stream/features';
import { DRIVEWAYS, KERB_RISE_M, metresPerUnit, PATH, PROFILES, ROAD, SHOULDER_M, SURFACES, walkOf, type Line } from './bins';
import { LEFT, RIGHT } from './town';

/** The metres along a way that the search reads from its junction, in steps; as WINDOW_M of roads/arms.ts, so a tile that holds the junction holds what it reads. */
const WINDOW_M = 16, STEP_M = 0.25;
/** A path at a road's edge without a sidewalk stands this far over the road's plane: the first triangle past the edge, which the road's tolerance lifts, stays under its tread. A way with a mouth stays on the plane, or it raises the through road's edge (roads/planes.ts). */
const OVER_PLANE_M = 0.08;
/** A way returns to its own heights at this grade at most, over this many metres at least; a change under SAME_M stays out. */
const RETURN_GRADE = 0.15, RETURN_LEAST_M = 2, SAME_M = 0.01, STOP_CLEAR_M = 2;

const smooth = (t: number) => t * t * (3 - 2 * t);

/** A way out of a vertex: `step` along the line. */
interface Way { line: Line; v: number; step: 1 | -1 }

/** Metres along a line at each vertex. */
function alongOf({ xy, z }: Line): number[] {
    const out = [0];
    for (let v = 1; v < z.length; v++) out.push(out[v - 1] + Math.hypot(xy[2 * v] - xy[2 * v - 2], xy[2 * v + 1] - xy[2 * v - 1]) * metresPerUnit(xy[2 * v + 1]));
    return out;
}

/** The point of a line `metres` along it from its first vertex: x, y and its segment with the share along that. */
function pointAlong({ xy }: Line, along: number[], metres: number): { x: number; y: number; v: number; u: number } {
    let v = 0;
    while (v + 2 < along.length && along[v + 1] < metres) v++;
    const u = Math.max(0, Math.min(1, (metres - along[v]) / ((along[v + 1] - along[v]) || 1)));
    return { x: xy[2 * v] + u * (xy[2 * v + 2] - xy[2 * v]), y: xy[2 * v + 1] + u * (xy[2 * v + 3] - xy[2 * v + 1]), v, u };
}

/** A world point's foot on the road ways within the window: the road's surface there, or nothing where the point lies past their outer edges. */
function surfaceAt(ways: Way[], x: number, y: number, unit: number): { z: number; walk: boolean } | undefined {
    for (const { line, v, step } of ways) {
        let metres = 0;
        for (let k = v; k + step >= 0 && k + step < line.z.length && metres < WINDOW_M; k += step) {
            const n = k + step, dx = (line.xy[2 * n] - line.xy[2 * k]) * unit, dy = (line.xy[2 * n + 1] - line.xy[2 * k + 1]) * unit, l = Math.hypot(dx, dy) || 1, ex = (x - line.xy[2 * k]) * unit, ey = (y - line.xy[2 * k + 1]) * unit;
            const u = (ex * dx + ey * dy) / l / l;
            metres += l;
            if (u < 0 || u > 1) continue;
            // Left of the line's own direction, which `step` turns: y runs south.
            const left = (step * (ex * dy - ey * dx)) / l, segment = Math.min(k, n), walks = line.sidewalks?.[segment] ?? 0, walk = walks & (left > 0 ? LEFT : RIGHT) ? walkOf(line) : 0;
            if (Math.abs(left) >= line.h + (walk || (SHOULDER_M[(Object.keys(SURFACES) as (keyof typeof SURFACES)[])[(line.code & 15) - 1]] ?? 0))) continue;
            const bank = line.plan ? line.plan.bank[k] + u * (line.plan.bank[n] - line.plan.bank[k]) : 0;
            return { z: line.z[k] + u * (line.z[n] - line.z[k]) + bank * Math.max(-line.h, Math.min(line.h, left)), walk: walk > 0 };
        }
    }
    return undefined;
}

/** What a way needs near its junction: from `from` metres along its line the height `z`, back to its own by `to`. */
interface Edit { at: number; from: number; to: number; z: number }

/** Where a lesser way leaves its roads, and the height it takes there. */
function editOf(way: Way, roads: Way[], along: number[], stops: Set<string>): Edit | undefined {
    const { line, v, step } = way, start = along[v], total = along[along.length - 1], room = step > 0 ? total - start : start;
    const out = (metres: number) => start + step * metres, unit = metresPerUnit(line.xy[2 * v + 1]);
    /** The road under the way at a share of its width across; `metres` from the junction. */
    const under = (metres: number) => {
        const { x, y, v: s } = pointAlong(line, along, out(metres)), dx = line.xy[2 * s + 2] - line.xy[2 * s], dy = line.xy[2 * s + 3] - line.xy[2 * s + 1], l = Math.hypot(dx, dy) || 1, across = line.h / unit / l;
        return [0, 1, -1].map(a => surfaceAt(roads, x - dy * across * a, y + dx * across * a, unit)).find(Boolean);
    };
    let metres = 0, held: ReturnType<typeof under>;
    for (; metres < Math.min(WINDOW_M, room); metres += STEP_M) {
        const hit = under(metres);
        if (!hit) break;
        held = hit;
    }
    if (!held || metres >= WINDOW_M || metres >= room) return undefined;
    // Past a next junction the heights are that junction's.
    let free = room;
    for (let k = v + step; k >= 0 && k < line.z.length; k += step) {
        if (stops.has(`${line.xy[2 * k]},${line.xy[2 * k + 1]}`) || k === 0 || k === line.z.length - 1) {
            free = Math.abs(along[k] - start);
            break;
        }
    }
    // A way keeps its own heights next to another junction: a mouth there reads them, up to about this far.
    const last = Math.min(free - STOP_CLEAR_M, WINDOW_M);
    if (metres + RETURN_LEAST_M > last) return undefined;
    const z = held.z + (held.walk ? KERB_RISE_M : line.kind === PATH ? OVER_PLANE_M : 0), p = pointAlong(line, along, out(metres));
    const change = z - (line.z[p.v] + p.u * (line.z[p.v + 1] - line.z[p.v]));
    if (Math.abs(change) < SAME_M) return undefined;
    return { at: start, from: out(metres), to: out(Math.min(last, metres + Math.max(RETURN_LEAST_M, Math.abs(change) / RETURN_GRADE))), z };
}

/** Cuts a line at these metres along it: the cuts become vertices on its curve. */
function cutLine(line: Line, cuts: number[], along: number[]): void {
    const wanted = [...new Set(cuts)].sort((a, b) => a - b), out: Line = { ...line, xy: [line.xy[0], line.xy[1]], z: [line.z[0]], anchors: [true], sidewalks: line.sidewalks && [], sideWays: line.sideWays && [line.sideWays[0]], plan: line.plan && { tangent: [line.plan.tangent[0]], grade: [line.plan.grade[0]], bank: [line.plan.bank[0]], widen: [line.plan.widen[0]] } };
    out.anchors![0] = line.anchors?.[0] ?? true;
    const push = (x: number, y: number, z: number, anchor: boolean, side: number, tangent: number, grade: number, bank: number, widen: number, walks: number | undefined) => {
        out.xy.push(x, y);
        out.z.push(z);
        out.anchors!.push(anchor);
        out.sideWays?.push(side);
        if (out.plan) {
            out.plan.tangent.push(tangent);
            out.plan.grade.push(grade);
            out.plan.bank.push(bank);
            out.plan.widen.push(widen);
        }
        if (walks !== undefined) out.sidewalks!.push(walks);
    };
    for (let v = 0, c = 0; v + 1 < line.z.length; v++) {
        const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), metres = along[v + 1] - along[v], plan = line.plan, walks = line.sidewalks?.[v];
        const span = plan && spanOf(bx - ax, by - ay, plan.tangent[v], plan.tangent[v + 1], line.z[v], line.z[v + 1], plan.grade[v], plan.grade[v + 1], metres);
        while (c < wanted.length && wanted[c] <= along[v] + 1e-3) c++;
        for (; c < wanted.length && wanted[c] < along[v + 1] - 1e-3; c++) {
            const u = (wanted[c] - along[v]) / metres, on = span && pointOn(span, u), mix = (p: number, q: number) => p + u * (q - p);
            push(on ? ax + on.x : mix(ax, bx), on ? ay + on.y : mix(ay, by), on?.z ?? mix(line.z[v], line.z[v + 1]), true, 0, on?.tangent ?? 0, on?.grade ?? 0, plan ? mix(plan.bank[v], plan.bank[v + 1]) : 0, plan ? mix(plan.widen[v], plan.widen[v + 1]) : 0, walks);
        }
        push(bx, by, line.z[v + 1], line.anchors?.[v + 1] ?? true, line.sideWays?.[v + 1] ?? 0, plan?.tangent[v + 1] ?? 0, plan?.grade[v + 1] ?? 0, plan?.bank[v + 1] ?? 0, plan?.widen[v + 1] ?? 0, walks);
    }
    Object.assign(line, { xy: out.xy, z: out.z, anchors: out.anchors, sidewalks: out.sidewalks, sideWays: out.sideWays, plan: out.plan });
}

/**
 * Puts each lesser way out of a road's junction on that road's outer edge where it leaves it, and back to its own
 * heights past that. A lesser way is a path, or a track or service way that the bake marks a side. A side road has a
 * mouth, whose faces meet the through road (roads/planes.ts).
 */
export function approaches(lines: Line[]): void {
    const counted = new Set(lines.filter(({ kind, gallery }) => (kind === ROAD || kind === PATH) && !gallery)), at = new Map<string, { line: Line; v: number }[]>(), stops = new Set<string>();
    for (const line of lines) line.z.forEach((_, v) => {
        const name = `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
        if (at.has(name)) stops.add(name);
        (at.get(name) ?? at.set(name, []).get(name)!).push({ line, v });
    });
    const edits = new Map<Line, Edit[]>(), alongs = new Map<Line, number[]>(), alongOfLine = (line: Line) => alongs.get(line) ?? alongs.set(line, alongOf(line)).get(line)!;
    for (const name of stops) {
        const ways = at.get(name)!.filter(({ line }) => counted.has(line)).flatMap(({ line, v }) => ([[-1, SIDE_BACK], [1, SIDE_ON]] as const).filter(([step]) => v + step >= 0 && v + step < line.z.length).map(([step, bit]) => ({ line, v, step, side: line.kind === PATH || !!((line.sideWays?.[v] ?? 0) & bit) })));
        const roads = ways.filter(({ side }) => !side);
        if (!roads.length) continue;
        for (const way of ways) {
            const type = Object.keys(PROFILES)[way.line.profile];
            if (!way.side || type === 'steps' || (way.line.kind === ROAD && !DRIVEWAYS.includes(type))) continue;
            const edit = editOf(way, roads, alongOfLine(way.line), stops);
            if (edit) (edits.get(way.line) ?? edits.set(way.line, []).get(way.line)!).push(edit);
        }
    }
    for (const [line, list] of edits) {
        cutLine(line, list.flatMap(({ from, to }) => [from, to]), alongOfLine(line));
        const along = alongOf(line);
        for (const { at: start, from, to, z } of list) {
            const nearest = (metres: number) => along.reduce((best, m, v) => (Math.abs(m - metres) < Math.abs(along[best] - metres) ? v : best), 0), fromV = nearest(from), toV = nearest(to);
            const change = z - line.z[fromV];
            const [lo, hi] = from < to ? [fromV, toV] : [toV, fromV], metres = Math.abs(to - from);
            for (let v = lo; v <= hi; v++) line.z[v] += change * (1 - smooth(Math.abs(along[v] - from) / metres));
            // Toward its junction a way runs from the junction's height to its new one.
            const startV = nearest(start), [p, q] = from < start ? [fromV, startV] : [startV, fromV];
            for (let v = p + 1; v < q; v++) line.z[v] = line.z[p] + (line.z[q] - line.z[p]) * (along[v] - along[p]) / (along[q] - along[p]);
            if (line.plan) for (let v = Math.max(1, Math.min(p, lo)); v < Math.min(line.z.length - 1, Math.max(q, hi) + 1); v++) line.plan.grade[v] = (line.z[v + 1] - line.z[v - 1]) / (along[v + 1] - along[v - 1]);
        }
    }
}
