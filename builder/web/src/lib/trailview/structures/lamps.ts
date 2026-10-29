// Street lamps along the roads of built-up places. The map holds few of them, and not in every region,
// so a rule stands them. PROTOTYPE (road study): from the stations of the fitted road (roads/stations.ts),
// so a lamp stands where the corridor draws its sidewalk: on one side every SPACING_M, on the sidewalk's
// outer half where the road has one, else BEHIND_M behind its shoulder where a building is near; its arm
// reaches over the road. None stands in a junction's mouth or on a crossing, in a driveway, on another
// road's carriageway, in a junction's corner, or in or by a building.
import { unitModel, type Build, type Kind } from '../kinds/kind';
import { LANTERN, streetLamp } from '../models/amenities';
import { insideRings, junctions, metresPerUnit, ringsDistance, ROAD, walkOf, type Line } from '../terrain/bins';
import { along, BUILT_M, builtNear, LEFT, near, RIGHT, townOf } from '../terrain/town';
import { classOf, SHOULDER_M, surfaceName } from '../roads/section';
import { stationsOf, type Station } from '../roads/stations';

/** The roads that have lamps. */
const LIT = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified', 'residential', 'living_street']);
/**
 * A lamp stands SPACING_M from the next, moved up to SEARCH_M along to a free place; on the sidewalk at
 * SIDEWALK_SHARE of its width, else BEHIND_M past the shoulder; CLEAR_M from a building's walls,
 * OFF_ROAD_M from another carriageway, MOUTH_M past the edge of a road that joins, DRIVE_M from a driveway.
 */
const SPACING_M = 32, SEARCH_M = 10, SIDEWALK_SHARE = 0.72, BEHIND_M = 0.5, CLEAR_M = 1.5, OFF_ROAD_M = 0.4, MOUTH_M = 4, DRIVE_M = 1.2, REACH_M = 600;

const lamp = unitModel(streetLamp, REACH_M);
/** The builds that have their lamps: one pass over all the roads of a node. */
const done = new WeakSet<Build>();

/** The kind of a lit road; `side` is kept for the registry, the lamps stand by the corridor's sections. */
export function lampKind(side: number): Kind {
    void side;
    return {
        build(_feature, out) {
            if (done.has(out)) return;
            done.add(out);
            streetLamps(out);
        },
        models: { lamp: lamp.spec },
        lights: { lamp: { colour: '#ffc98a', radius: 16, at: LANTERN, housed: true } },
    };
}

/** The station a share u of the way from s to t. */
function lerp(s: Station, t: Station, u: number): Station {
    const f = (p: number, q: number) => p + (q - p) * u;
    return { ...s, x: f(s.x, t.x), y: f(s.y, t.y), z: f(s.z, t.z), ax: f(s.ax, t.ax), ay: f(s.ay, t.ay), along: f(s.along, t.along), j: undefined };
}

/** The station `metres` along a line's stations. */
function stationAt(st: Station[], metres: number): Station | undefined {
    let k = 0;
    while (k + 1 < st.length && st[k + 1].along < metres) k++;
    if (k + 1 >= st.length || metres < 0) return undefined;
    return lerp(st[k], st[k + 1], (metres - st[k].along) / Math.max(st[k + 1].along - st[k].along, 1e-6));
}

function streetLamps(out: Build): void {
    const town = townOf(out.buildings), roads = out.ways.filter(line => line.kind === ROAD);
    const joins = [...junctions(out.ways).values()].filter(j => j.ways > 2);
    const walks = out.walks ?? [];
    for (const line of roads) {
        if (!LIT.has(classOf(line)) || line.gallery) continue;
        const st = stationsOf(line), total = st[st.length - 1].along, walk = walkOf(line);
        // Where a lamp may stand at `metres` along: its place, side and yaw, or undefined.
        const spot = (metres: number) => {
            const s = stationAt(st, metres);
            if (!s) return undefined;
            const unit = metresPerUnit(s.y);
            for (const side of [-1, 1]) {
                const walked = (line.sidewalks?.[s.v] ?? 0) & (side > 0 ? LEFT : RIGHT);
                if (!walked && !builtNear(town, s.x, s.y, BUILT_M / unit)) continue;
                const a = line.h + (walked ? walk * SIDEWALK_SHARE : (SHOULDER_M[surfaceName(line)] ?? 0) + BEHIND_M);
                const x = s.x + side * s.ax * a, y = s.y + side * s.ay * a;
                if (!out.owns(x, y) || out.cornered(x, y)) continue;
                if (joins.some(j => Math.hypot(j.x - x, j.y - y) * unit < j.h + walk + MOUTH_M)) continue;
                if (walks.some(w => Math.hypot(w.xy[2] - x, w.xy[3] - y) * unit < w.h + walk + DRIVE_M)) continue;
                if (inTraffic(roads, line, x, y, unit)) continue;
                let walled = false;
                near(town, x, y, x, y, CLEAR_M / unit, b => (walled ||= insideRings(out.buildings[b].rings, x, y) || ringsDistance(out.buildings[b].rings, x, y) * unit < CLEAR_M));
                if (walled) continue;
                // The arm reaches toward −y of the model: over the road, toward the line's middle.
                return { x, y, yaw: Math.atan2(-side * s.ax, -side * s.ay) };
            }
            return undefined;
        };
        for (let metres = SPACING_M / 2; metres < total; ) {
            let placed = false;
            for (let d = 0; d <= SEARCH_M && !placed; d += 2) {
                for (const m of d ? [metres + d, metres - d] : [metres]) {
                    const it = spot(m);
                    if (!it) continue;
                    out.place({ ...it, height: lamp.height, model: 'lamp' });
                    metres = m + SPACING_M;
                    placed = true;
                    break;
                }
            }
            if (!placed) metres += SPACING_M;
        }
    }
}

/** Whether a point lies within OFF_ROAD_M of the carriageway of a road other than `own`. */
function inTraffic(roads: Line[], own: Line, x: number, y: number, unit: number): boolean {
    return roads.some(line => {
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
