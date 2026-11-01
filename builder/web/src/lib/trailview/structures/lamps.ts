// Street lamps along the roads of built-up places. The map holds few of them, and not in every region,
// so a rule stands them: along a lit road, every SPACING_M on its right, where a building is near. A lamp
// stands on the road's sidewalk where it has one, BEHIND_KERB_M behind its kerb, where the road's piece there
// has one on its right; on another road, where the town rule finds a building at its foot. None stands within OFF_ROAD_M of
// a carriageway, in a junction's corner or within JUNCTION_M of a junction of three or more ways, nor in a
// building or by its walls.
import { facing, nearestWay, unitModel, type Build, type Kind, type Placed } from '../kinds/kind';
import { LANTERN, streetLamp } from '../models/amenities';
import { halfWidth, insideRings, junctions, metresPerUnit, ringsDistance, ROAD, sidewalkOf, surfaceOf, type Line } from '../terrain/bins';
import { along, BUILT_M, builtNear, near, RIGHT, townOf } from '../terrain/town';

/** The roads that have lamps; a lamp stands this many metres apart from the next, at least CLEAR_M from a building's walls and OFF_ROAD_M from every carriageway, less than its place behind its own kerb. */
export const LIT = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified', 'residential', 'living_street']);
const SPACING_M = 32, BEHIND_KERB_M = 0.5, CLEAR_M = 2, OFF_ROAD_M = 0.25, JUNCTION_M = 10, REACH_M = 600;

export const lamp = unitModel(streetLamp, REACH_M);
/** The carriageways of a node's ways, and their junctions. */
const traffic = new WeakMap<Line[], { roads: Line[]; joins: { x: number; y: number }[] }>();

/** The kind of a lit road: lamps `side` metres from its middle where it has no sidewalk. */
export function lampKind(side: number): Kind {
    return {
        build(feature, out) {
            if (!out.corridors && LIT.has(feature.tags.highway) && !(feature.tags.bridge && feature.tags.bridge !== 'no')) lamps(feature, out, side);
        },
        models: { lamp: lamp.spec },
        lights: { lamp: { colour: '#ffc98a', radius: 16, at: LANTERN, housed: true } },
    };
}

function lamps({ tags, parts }: Placed, out: Build, side: number): void {
    const town = townOf(out.buildings), surface = surfaceOf(tags), h = halfWidth(tags, surface), sidewalk = sidewalkOf(tags, surface);
    for (const { xy } of parts) {
        // Metres along the line and where the next lamp stands.
        let done = 0, next = SPACING_M / 2;
        for (let i = 0; i + 3 < xy.length; i += 2) {
            const ax = xy[i], ay = xy[i + 1], dx = xy[i + 2] - ax, dy = xy[i + 3] - ay, unit = metresPerUnit(ay), length = Math.hypot(dx, dy) * unit;
            const across = (sidewalk ? h + BEHIND_KERB_M : side) / unit / Math.hypot(dx, dy);
            for (; next <= done + length; next += SPACING_M) {
                // On the right of the way, whose y runs south.
                const t = (next - done) / length, x = ax + t * dx - dy * across, y = ay + t * dy + dx * across;
                if (!out.owns(x, y)) continue;
                const way = sidewalk ? nearestWay(out.ways, ax + t * dx, ay + t * dy) : undefined;
                if (!(way ? (way.line.sidewalks?.[way.v] ?? 0) & RIGHT : builtNear(town, ax + t * dx, ay + t * dy, BUILT_M / unit)) || out.cornered(x, y) || inTraffic(out.ways, x, y, unit)) continue;
                let walled = false;
                near(town, x, y, x, y, CLEAR_M / unit, b => (walled ||= insideRings(out.buildings[b].rings, x, y) || ringsDistance(out.buildings[b].rings, x, y) * unit < CLEAR_M));
                if (walled) continue;
                out.place({ x, y, yaw: facing(out.ways, x, y) ?? 0, height: lamp.height, model: 'lamp' });
            }
            done += length;
        }
    }
}

/** Whether a point lies on a road's carriageway or within OFF_ROAD_M of it, or within JUNCTION_M of a junction of three or more ways. */
function inTraffic(ways: Line[], x: number, y: number, unit: number): boolean {
    let known = traffic.get(ways);
    if (!known) {
        const roads = ways.filter(line => line.kind === ROAD);
        traffic.set(ways, (known = { roads, joins: [...junctions(roads).values()].filter(j => j.ways > 2) }));
    }
    if (known.joins.some(j => Math.hypot(j.x - x, j.y - y) * unit < JUNCTION_M)) return true;
    return known.roads.some(line => {
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
