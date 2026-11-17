/** Groups the places of one place tile into badges for each whole zoom, from the tile and the kinds that are on alone. */
import { tileColumn, tileRow } from './place-index';
import type { PlaceCategory } from './poi-kinds';

export type TilePlace = { id: number; kind: string; category: PlaceCategory; name: string; lon: number; lat: number; ele?: number };
export type Badge = { key: number; parent?: number; lon: number; lat: number; places: TilePlace[] };

export const FIRST_LEVEL = 12;
const LAST_LEVEL = 20;
const PEAKS = new Set(['summit', 'pass']);

/** The badges of each level z from 12 to 20 at index z - FIRST_LEVEL. */
export function groupTile(places: TilePlace[], on: PlaceCategory[]): Badge[][] {
    const shown = places.filter(place => on.includes(place.category)).sort((a, b) => a.id - b.id);
    const first = firstLevels(shown);
    // A badge's first place is its seat: the place that shows first, and of those the smallest id. The seat goes
    // only with the whole badge, so a badge keeps its key and its position at each level where it exists.
    shown.sort((a, b) => first.get(a)! - first.get(b)! || a.id - b.id);
    // Positions in pixels of a world 512 px wide at zoom 0; the reach at level z is its pixels / 2^z.
    const position = new Map(shown.map(place => [place, [tileColumn(place.lon, 0) * 512, tileRow(place.lat, 0) * 512]]));
    const levels: Badge[][] = [shown.map(place => ({ key: place.id, lon: place.lon, lat: place.lat, places: [place] }))];
    for (let z = LAST_LEVEL; z >= FIRST_LEVEL; z--) {
        const reach = (z >= 16 ? 32 : 64) / 2 ** z;
        const parts = levels[0].flatMap(from => {
            const places = from.places.filter(place => first.get(place)! <= z);
            if (!places.length) return [];
            const [x, y] = position.get(places[0])!;
            return [{ from, seat: places[0], places, x, y, column: Math.floor(x / reach), row: Math.floor(y / reach) }];
        });
        // Cells as wide as the reach: a badge reaches only into its own cell and the eight around it.
        // A column and a row stay below 2^24 up to level 20, so the number of a cell is unique.
        const cells = new Map<number, typeof parts>();
        const cell = (column: number, row: number) => column * 2 ** 26 + row;
        for (const part of parts) {
            const key = cell(part.column, part.row);
            if (cells.has(key)) cells.get(key)!.push(part);
            else cells.set(key, [part]);
        }
        const taken = new Set<object>();
        const level: Badge[] = [];
        // The badges of a level stand in the order of their seats, so the parts need no sort.
        for (const part of parts) {
            if (taken.has(part)) continue;
            taken.add(part);
            const badge: Badge = { key: part.seat.id, lon: part.seat.lon, lat: part.seat.lat, places: [...part.places] };
            part.from.parent = badge.key;
            for (let column = part.column - 1; column <= part.column + 1; column++) {
                for (let row = part.row - 1; row <= part.row + 1; row++) {
                    for (const other of cells.get(cell(column, row)) ?? []) {
                        if (taken.has(other) || Math.abs(other.x - part.x) > reach || Math.abs(other.y - part.y) > reach) continue;
                        taken.add(other);
                        badge.places.push(...other.places);
                        other.from.parent = badge.key;
                    }
                }
            }
            level.push(badge);
        }
        levels.unshift(level);
    }
    // The last entry holds the single places that level 20 groups.
    return levels.slice(0, -1);
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
