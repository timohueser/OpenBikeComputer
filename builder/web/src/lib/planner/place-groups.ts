/**
 * Groups the places of one place tile into spots for each whole zoom, from the tile and the kinds alone, and makes the
 * marks of the map from them: from level 12 a disc for each spot of the highlighted kind, which is grouped alone, and
 * from level 16 a row of icons, one for each kind, for each spot of the other kinds that are on.
 */
import { tileColumn, tileRow } from './place-index';
import { categoryIds, type PlaceCategory } from './poi-kinds';

export type TilePlace = { id: number; kind: string; category: PlaceCategory; name?: string; lon: number; lat: number; ele?: number };
export type Spot = { key: number; lon: number; lat: number; places: TilePlace[] };
/** What the map draws for a spot: a disc of one kind, or a row with one icon for each kind of the spot. */
export type Mark = { key: string; lon: number; lat: number; disc: boolean; icons: { category: PlaceCategory; places: TilePlace[] }[] };

export const FIRST_LEVEL = 12;
export const LAST_LEVEL = 20;
/** The first level of the rows of the quiet kinds. */
export const ROW_LEVEL = 16;
/** The deepest zoom of a fit of the map to the places of a mark. */
export const FIT_LEVEL = 17;
const PEAKS = new Set(['summit', 'pass']);

/** The spots of each level z from 12 to 20 at index z - FIRST_LEVEL. */
export function groupTile(places: TilePlace[], on: PlaceCategory[]): Spot[][] {
    const shown = places.filter(place => on.includes(place.category)).sort((a, b) => a.id - b.id);
    const first = firstLevels(shown);
    // A spot's first place is its seat: the place that shows first, and of those the smallest id. The seat goes
    // only with the whole spot, so a spot keeps its key and its position at each level where it exists.
    shown.sort((a, b) => first.get(a)! - first.get(b)! || a.id - b.id);
    // Positions in pixels of a world 512 px wide at zoom 0; the reach at level z is its pixels / 2^z.
    const position = new Map(shown.map(place => [place, [tileColumn(place.lon, 0) * 512, tileRow(place.lat, 0) * 512]]));
    const levels: Spot[][] = [shown.map(place => ({ key: place.id, lon: place.lon, lat: place.lat, places: [place] }))];
    for (let z = LAST_LEVEL; z >= FIRST_LEVEL; z--) {
        const reach = (z >= 16 ? 32 : 64) / 2 ** z;
        const parts = levels[0].flatMap(from => {
            const places = from.places.filter(place => first.get(place)! <= z);
            if (!places.length) return [];
            const [x, y] = position.get(places[0])!;
            return [{ from, seat: places[0], places, x, y, column: Math.floor(x / reach), row: Math.floor(y / reach) }];
        });
        // Cells as wide as the reach: a spot reaches only into its own cell and the eight around it.
        // A column and a row stay below 2^24 up to level 20, so the number of a cell is unique.
        const cells = new Map<number, typeof parts>();
        const cell = (column: number, row: number) => column * 2 ** 26 + row;
        for (const part of parts) {
            const key = cell(part.column, part.row);
            if (cells.has(key)) cells.get(key)!.push(part);
            else cells.set(key, [part]);
        }
        const taken = new Set<object>();
        const level: Spot[] = [];
        // The spots of a level stand in the order of their seats, so the parts need no sort.
        for (const part of parts) {
            if (taken.has(part)) continue;
            taken.add(part);
            const spot: Spot = { key: part.seat.id, lon: part.seat.lon, lat: part.seat.lat, places: [...part.places] };
            for (let column = part.column - 1; column <= part.column + 1; column++) {
                for (let row = part.row - 1; row <= part.row + 1; row++) {
                    for (const other of cells.get(cell(column, row)) ?? []) {
                        if (taken.has(other) || Math.abs(other.x - part.x) > reach || Math.abs(other.y - part.y) > reach) continue;
                        taken.add(other);
                        spot.places.push(...other.places);
                    }
                }
            }
            level.push(spot);
        }
        levels.unshift(level);
    }
    // The last entry holds the single places that level 20 groups.
    return levels.slice(0, -1);
}

/** The discs of the highlighted kind at each level, at index z - FIRST_LEVEL. */
export function discMarks(places: TilePlace[], highlight: PlaceCategory): Mark[][] {
    return groupTile(places, [highlight]).map(level => level.map(spot =>
        ({ key: `d${spot.key}`, lon: spot.lon, lat: spot.lat, disc: true, icons: [{ category: highlight, places: spot.places }] })));
}

/** The rows of the quiet `kinds` at each level, at index z - FIRST_LEVEL; the levels below 16 have none. */
export function rowMarks(places: TilePlace[], kinds: PlaceCategory[]): Mark[][] {
    return groupTile(places, kinds).map((level, index) => index + FIRST_LEVEL < ROW_LEVEL ? [] : level.map(spot => ({
        key: `r${spot.key}`, lon: spot.lon, lat: spot.lat, disc: false,
        icons: categoryIds.flatMap(category => {
            const of = spot.places.filter(place => place.category === category);
            return of.length ? [{ category, places: of }] : [];
        }),
    })));
}

/** What a click on the places of one button does at a level: open the place, list the places, or zoom to them. */
export function opens(places: TilePlace[], level: number): 'place' | 'list' | 'zoom' {
    return places.length === 1 ? 'place' : places.length <= 8 || level >= FIT_LEVEL ? 'list' : 'zoom';
}

/**
 * The first level of each place: 12, but for a summit or a pass the lowest level z below 16 at which it is
 * the highest of its kind in its tile of zoom z + 1, else 16. No height ranks lowest; a tie goes to the smaller id.
 */
function firstLevels(places: TilePlace[]): Map<TilePlace, number> {
    const first = new Map(places.map(place => [place, PEAKS.has(place.kind) ? 16 : FIRST_LEVEL]));
    for (let z = FIRST_LEVEL; z < 16; z++) {
        const highest = new Map<string, TilePlace>();
        for (const place of places) {
            if (!PEAKS.has(place.kind)) continue;
            const square = `${place.kind}/${Math.floor(tileColumn(place.lon, z + 1))}/${Math.floor(tileRow(place.lat, z + 1))}`;
            const best = highest.get(square);
            // Places come sorted by id, so a tie keeps the place already found.
            if (!best || (place.ele ?? -Infinity) > (best.ele ?? -Infinity)) highest.set(square, place);
        }
        for (const place of highest.values()) first.set(place, Math.min(first.get(place)!, z));
    }
    return first;
}
