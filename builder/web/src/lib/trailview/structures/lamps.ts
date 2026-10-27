// Street lamps along the roads of built-up places. The map holds few of them, and not in every region,
// so a rule stands them: along a lit road, every SPACING_M on one side, where a building is near.
import { facing, unitModel, type Build, type Kind, type Placed } from '../kinds/kind';
import { LANTERN, streetLamp } from '../models/amenities';
import { insideRings, metresPerUnit, ringsDistance, type Shapes } from '../terrain/bins';

/**
 * The roads that have lamps; a lamp stands this many metres apart from the next, only within about BUILT_M
 * of a building, and at least CLEAR_M from its walls.
 */
const LIT = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified', 'residential', 'living_street']);
const SPACING_M = 32, BUILT_M = 35, CLEAR_M = 2, REACH_M = 600;

const lamp = unitModel(streetLamp, REACH_M);
type Buildings = Shapes['buildings'];
/** The buildings of a tile by the cells of `size`, about BUILT_M, that their bounds touch. */
interface Cells { size: number; cells: Map<string, Buildings> }
const built = new WeakMap<Buildings, Cells>();

/** The kind of a lit road: lamps `side` metres from its middle. */
export function lampKind(side: number): Kind {
    return {
        build(feature, out) {
            if (LIT.has(feature.tags.highway) && !(feature.tags.bridge && feature.tags.bridge !== 'no')) lamps(feature, out, side);
        },
        models: { lamp: lamp.spec },
        lights: { lamp: { colour: '#ffc98a', radius: 16, at: LANTERN, housed: true } },
    };
}

function lamps({ parts }: Placed, out: Build, side: number): void {
    const cells = cellsOf(out.buildings);
    for (const { xy } of parts) {
        // Metres along the line and where the next lamp stands.
        let done = 0, next = SPACING_M / 2;
        for (let i = 0; i + 3 < xy.length; i += 2) {
            const ax = xy[i], ay = xy[i + 1], dx = xy[i + 2] - ax, dy = xy[i + 3] - ay, unit = metresPerUnit(ay), length = Math.hypot(dx, dy) * unit;
            for (; next <= done + length; next += SPACING_M) {
                const t = (next - done) / length, offset = side / unit / Math.hypot(dx, dy);
                // On the right of the way, in world units, whose y runs south.
                const x = ax + t * dx - dy * offset, y = ay + t * dy + dx * offset;
                if (!out.owns(x, y)) continue;
                const near = nearBuildings(cells, x, y);
                if (!near.length || near.some(({ rings }) => insideRings(rings, x, y) || ringsDistance(rings, x, y) * unit < CLEAR_M)) continue;
                out.place({ x, y, yaw: facing(out.ways, x, y) ?? 0, height: lamp.height, model: 'lamp' });
            }
            done += length;
        }
    }
}

function cellsOf(buildings: Buildings): Cells {
    let index = built.get(buildings);
    if (!index) {
        // One size for the tile: a size at each building's own y would shift its cells against the lamps' cells.
        const y = buildings.flatMap(({ rings }) => rings).find(ring => ring.length)?.[1] ?? 0;
        index = { size: BUILT_M / metresPerUnit(y), cells: new Map() };
        const { size, cells } = index;
        for (const building of buildings) {
            let [x0, x1, y0, y1] = [Infinity, -Infinity, Infinity, -Infinity];
            for (const ring of building.rings) for (let i = 0; i < ring.length; i += 2) [x0, x1, y0, y1] = [Math.min(x0, ring[i]), Math.max(x1, ring[i]), Math.min(y0, ring[i + 1]), Math.max(y1, ring[i + 1])];
            for (let cx = Math.floor(x0 / size); cx <= Math.floor(x1 / size); cx++) for (let cy = Math.floor(y0 / size); cy <= Math.floor(y1 / size); cy++) {
                const key = `${cx},${cy}`, list = cells.get(key);
                if (list) list.push(building);
                else cells.set(key, [building]);
            }
        }
        built.set(buildings, index);
    }
    return index;
}

/** The buildings in the cell of x, y and the cells next to it. */
function nearBuildings({ size, cells }: Cells, x: number, y: number): Buildings {
    const cx = Math.floor(x / size), cy = Math.floor(y / size), near = new Set<Buildings[number]>();
    for (const dx of [-1, 0, 1]) for (const dy of [-1, 0, 1]) for (const building of cells.get(`${cx + dx},${cy + dy}`) ?? []) near.add(building);
    return [...near];
}
