// Street lamps along the roads of built-up places. The map holds few of them, and not in every region,
// so a rule stands them: along a lit road, every SPACING_M on one side, where a building is near.
import { facing, unitModel, type Build, type Kind, type Placed } from '../kinds/kind';
import { streetLamp } from '../models/amenities';
import { metresPerUnit, type Shapes } from '../terrain/bins';

/** The roads that have lamps; a lamp stands this many metres apart from the next, and only within about BUILT_M of a building. */
const LIT = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified', 'residential', 'living_street']);
const SPACING_M = 32, BUILT_M = 35, REACH_M = 600;

const lamp = unitModel(streetLamp, REACH_M);
/** The cells of BUILT_M that hold a vertex of a building, by the buildings of a tile. */
const built = new WeakMap<Shapes['buildings'], Set<string>>();

/** The kind of a lit road: lamps `side` metres from its middle. */
export function lampKind(side: number): Kind {
    return {
        build(feature, out) {
            if (LIT.has(feature.tags.highway) && !(feature.tags.bridge && feature.tags.bridge !== 'no')) lamps(feature, out, side);
        },
        models: { lamp: lamp.spec },
        lights: { lamp: { colour: '#ffc98a', radius: 9 } },
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
                if (!out.owns(x, y) || !near(cells, x, y)) continue;
                out.place({ x, y, yaw: facing(out.ways, x, y) ?? 0, height: lamp.height, model: 'lamp' });
            }
            done += length;
        }
    }
}

function cellsOf(buildings: Shapes['buildings']): Set<string> {
    let cells = built.get(buildings);
    if (!cells) {
        cells = new Set();
        for (const { rings } of buildings) for (const ring of rings) for (let i = 0; i < ring.length; i += 2) cells.add(cell(ring[i], ring[i + 1]));
        built.set(buildings, cells);
    }
    return cells;
}

const cell = (x: number, y: number) => {
    const size = BUILT_M / metresPerUnit(y);
    return `${Math.floor(x / size)},${Math.floor(y / size)}`;
};

/** Whether a building is in the cell of x, y or a cell next to it. */
function near(cells: Set<string>, x: number, y: number): boolean {
    const size = BUILT_M / metresPerUnit(y);
    for (const dx of [-1, 0, 1]) for (const dy of [-1, 0, 1]) if (cells.has(cell(x + dx * size, y + dy * size))) return true;
    return false;
}
