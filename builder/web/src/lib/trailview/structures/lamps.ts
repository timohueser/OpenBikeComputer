// Street lamps along the roads of built-up places. The map holds few of them, and not in every region,
// so a rule stands them: along a lit road, every SPACING_M on one side, where a building is near. A lamp
// stands on the road's sidewalk where it has one, KERB_M behind its kerb: the town rule of the sidewalks
// decides its side at its foot. None stands in a junction's corner, where the kerb turns off its road.
import { facing, unitModel, type Build, type Kind, type Placed } from '../kinds/kind';
import { streetLamp } from '../models/amenities';
import { halfWidth, metresPerUnit, sidewalkOf, surfaceOf } from '../terrain/bins';
import { BUILT_M, RIGHT, sidesNear, townOf } from '../terrain/town';

/** The roads that have lamps; a lamp stands this many metres apart from the next. */
const LIT = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified', 'residential', 'living_street']);
const SPACING_M = 32, KERB_M = 0.5, REACH_M = 600;

const lamp = unitModel(streetLamp, REACH_M);

/** The kind of a lit road: lamps `side` metres from its middle where it has no sidewalk. */
export function lampKind(side: number): Kind {
    return {
        build(feature, out) {
            if (LIT.has(feature.tags.highway) && !(feature.tags.bridge && feature.tags.bridge !== 'no')) lamps(feature, out, side);
        },
        models: { lamp: lamp.spec },
        lights: { lamp: { colour: '#ffc98a', radius: 9 } },
    };
}

function lamps({ tags, parts }: Placed, out: Build, side: number): void {
    const town = townOf(out.buildings), surface = surfaceOf(tags), h = halfWidth(tags, surface), sidewalk = sidewalkOf(tags, surface);
    for (const { xy } of parts) {
        // Metres along the line and where the next lamp stands.
        let done = 0, next = SPACING_M / 2;
        for (let i = 0; i + 3 < xy.length; i += 2) {
            const ax = xy[i], ay = xy[i + 1], dx = xy[i + 2] - ax, dy = xy[i + 3] - ay, unit = metresPerUnit(ay), length = Math.hypot(dx, dy) * unit;
            const across = (sidewalk ? h + KERB_M : side) / unit / Math.hypot(dx, dy);
            for (; next <= done + length; next += SPACING_M) {
                // On the right of the way (world y runs south), or on its left where only that side has a sidewalk.
                const t = (next - done) / length, spot = (sign: number): [number, number] => [ax + t * dx - dy * across * sign, ay + t * dy + dx * across * sign];
                if (!out.owns(...spot(1)) && !out.owns(...spot(-1))) continue;
                const sides = sidesNear(town, ax + t * dx, ay + t * dy, dx, dy, BUILT_M / unit);
                const [x, y] = spot(sidewalk && !(sides & RIGHT) ? -1 : 1);
                if (!sides || !out.owns(x, y) || out.cornered(x, y)) continue;
                out.place({ x, y, yaw: facing(out.ways, x, y) ?? 0, height: lamp.height, model: 'lamp' });
            }
            done += length;
        }
    }
}
