// Street lamps from the stations of a lit road: where the corridor draws its sidewalk or behind its shoulder.
import { BRIDGE, cores, insideRings, junctions, metresPerUnit, PATH, ringsDistance, ROAD, TUNNEL, walkOf, type Line } from '../terrain/bins';
import { halfAt, partnersOf } from '../terrain/corners';
import { along, BUILT_M, builtNear, LEFT, near, RIGHT, townOf, type Town } from '../terrain/town';
import { LANTERN, streetLamp } from '../models/amenities';
import { unitModel, type Build, type Kind, type Placement } from '../kinds/kind';
import { classOf, shoulderOf } from './section';
import { stationAt, stationsOf, type Station } from './stations';

/**
 * A lamp stands SPACING_M from the next, moved up to SEARCH_M along to a free place: on the sidewalk at
 * SIDEWALK_SHARE of its width, else BEHIND_M past the shoulder; CLEAR_M from a building's walls,
 * OFF_ROAD_M from another carriageway, MOUTH_M past the edge of a way that joins,
 * and APART_M from every other lamp of its tile: each way starts its own count of SPACING_M.
 */
const SPACING_M = 32, SEARCH_M = 10, SIDEWALK_SHARE = 0.72, BEHIND_M = 0.5, CLEAR_M = 1.5, OFF_ROAD_M = 0.4, MOUTH_M = 4, APART_M = SPACING_M / 2;

/** The roads that have lamps, and how far from the eye a lamp draws. */
const LIT = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified', 'residential', 'living_street']);

const LAMP_REACH_M = 600;
const lamp = unitModel(streetLamp, LAMP_REACH_M);

/** The kind of the lit roads. `streetLamps` places their lamps, from the road's stations. */
export const lampKind: Kind = {
    build() {},
    models: { lamp: lamp.spec },
    lights: { lamp: { colour: '#ffc98a', radius: 16, at: LANTERN, housed: true } },
};

/** The lamps of each tile, made once: the stations of its lit roads are many, its lamps few. */
const planned = new WeakMap<Line[], Placement[]>();

/** What decides the place of a lamp. */
interface Places { out: Pick<Build, 'buildings' | 'cornered'>; town: Town; ways: Line[]; mouths: { x: number; y: number; h: number }[]; lamps: Placement[] }

/** Whether a point lies within OFF_ROAD_M of a carriageway other than `own`. */
function inTraffic(ways: Line[], own: Line, x: number, y: number, unit: number): boolean {
    return ways.some(line => {
        if (line === own) return false;
        const reach = (line.h + OFF_ROAD_M) / unit;
        for (let v = 0; v + 1 < line.z.length; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4);
            if (Math.min(ax, bx) > x + reach || Math.max(ax, bx) < x - reach || Math.min(ay, by) > y + reach || Math.max(ay, by) < y - reach) continue;
            const t = along(ax, ay, bx, by, x, y);
            if (Math.hypot(ax + t * (bx - ax) - x, ay + t * (by - ay) - y) < reach) return true;
        }
        return false;
    });
}

/** Whether a lamp may stand at world point x, y beside `line`, whose sidewalk is `walk` metres wide. */
function free({ out, town, ways, mouths, lamps }: Places, line: Line, walk: number, x: number, y: number, unit: number): boolean {
    if (out.cornered(x, y) || mouths.some(j => Math.hypot(j.x - x, j.y - y) * unit < j.h + walk + MOUTH_M)) return false;
    if (lamps.some(lamp => Math.hypot(lamp.x - x, lamp.y - y) * unit < APART_M)) return false;
    if (inTraffic(ways, line, x, y, unit)) return false;
    let walled = false;
    near(town, x, y, x, y, CLEAR_M / unit, b => (walled ||= insideRings(out.buildings[b].rings, x, y) || ringsDistance(out.buildings[b].rings, x, y) * unit < CLEAR_M));
    return !walled;
}

/** The place of a lamp `metres` along stations `st` of `line`, on its first free side, or undefined. */
function spot(places: Places, line: Line, st: Station[], metres: number): Placement | undefined {
    if (metres < 0 || metres > st[st.length - 1].along) return undefined;
    const s = stationAt(st, metres), unit = metresPerUnit(s.y), walk = walkOf(line);
    for (const side of [-1, 1]) {
        const walked = (line.sidewalks?.[s.v] ?? 0) & (side > 0 ? LEFT : RIGHT), a = s.h + (walked ? walk * SIDEWALK_SHARE : shoulderOf(line) + BEHIND_M);
        const x = s.x + side * s.ax * a, y = s.y + side * s.ay * a;
        if ((!walked && !builtNear(places.town, s.x, s.y, BUILT_M / unit)) || !free(places, line, walk, x, y, unit)) continue;
        // The arm reaches toward −y of the model: over the road, toward the line's middle.
        return { x, y, yaw: Math.atan2(-side * s.ax, -side * s.ay), height: lamp.height, model: 'lamp' };
    }
    return undefined;
}

/** The lamps of a tile's lit roads. */
function lampsOf(out: Pick<Build, 'buildings' | 'cornered'>, lines: Line[]): Placement[] {
    const partners = partnersOf(lines), ways = cores(lines), roads = lines.filter(line => line.kind === ROAD && !line.gallery && LIT.has(classOf(line)) && line.z.length > 1);
    // A road can run straight into a bridge or a tunnel, a node of two ways: each end of one is a mouth.
    const ends = lines.filter(({ kind }) => kind === BRIDGE || kind === TUNNEL).flatMap(({ xy, h }) => [{ x: xy[0], y: xy[1], h }, { x: xy[xy.length - 2], y: xy[xy.length - 1], h }]);
    const joins = [...junctions([...ways, ...lines.filter(({ kind }) => kind === PATH)]).values()].filter(j => j.ways > 2), lamps: Placement[] = [];
    const places = { out, town: townOf(out.buildings), ways, mouths: [...joins, ...ends], lamps };
    for (const line of roads) {
        const st = stationsOf(line, v => halfAt(line, v, partners)), total = st[st.length - 1].along;
        for (let metres = SPACING_M / 2; metres < total; ) {
            let placed: Placement | undefined;
            for (let d = 0; d <= SEARCH_M && !placed; d += 2) {
                for (const m of d ? [metres + d, metres - d] : [metres]) {
                    placed = spot(places, line, st, m);
                    if (placed) {
                        lamps.push(placed);
                        metres = m;
                        break;
                    }
                }
            }
            metres += SPACING_M;
        }
    }
    return lamps;
}

/** Stands the lamps of a tile that a node owns. `lines` are the tile's. */
export function streetLamps(out: Pick<Build, 'owns' | 'place' | 'buildings' | 'cornered'>, lines: Line[]): void {
    const lamps = planned.get(lines) ?? planned.set(lines, lampsOf(out, lines)).get(lines)!;
    for (const lamp of lamps) if (out.owns(lamp.x, lamp.y)) out.place(lamp);
}
