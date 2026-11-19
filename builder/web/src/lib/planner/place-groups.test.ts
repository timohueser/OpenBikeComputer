import { describe, expect, it } from 'vitest';
import { FIRST_LEVEL, discMarks, groupTile, opens, rowMarks, type Spot, type TilePlace } from './place-groups';
import { tileColumn, tileRow } from './place-index';
import { categoryIds, poiKinds, type PlaceCategory } from './poi-kinds';

const kinds = Object.keys(poiKinds);
const place = (id: number, kind: string, lon: number, lat: number, ele?: number): TilePlace =>
    ({ id, kind, category: poiKinds[kind].category, name: `${kind} ${id}`, lon, lat, ...ele !== undefined && { ele } });

/** Seeded places in the zoom-11 tile 11/1071/719, most of them in a town of about 1 km. */
function town(count: number, seed: number, west = 8.262, south = 47.042): TilePlace[] {
    const random = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
    return Array.from({ length: count }, (_, i) => {
        const size = random() < .7 ? .012 : .11;
        const kind = kinds[Math.floor(random() * kinds.length)];
        return place(1000 + i * 7, kind, west + random() * size, south + random() * size, random() < .8 ? Math.round(random() * 3000) : undefined);
    });
}

const levels = groupTile(town(400, 7), categoryIds);
const pixels = (spot: Spot, z: number) => [tileColumn(spot.lon, z) * 512, tileRow(spot.lat, z) * 512];

describe('place groups', () => {
    it('gives the same spots for the same places in another order', () => {
        const places = town(400, 7);
        expect(groupTile([...places].reverse(), categoryIds)).toEqual(groupTile(places, categoryIds));
    });

    it('keeps no two spots of a level inside the reach of each other', () => {
        for (let z = FIRST_LEVEL; z <= 20; z++) {
            const reach = z >= 16 ? 32 : 64, spots = levels[z - FIRST_LEVEL].map(spot => pixels(spot, z));
            const close = spots.flatMap((a, i) => spots.slice(i + 1).filter(b => Math.abs(a[0] - b[0]) <= reach && Math.abs(a[1] - b[1]) <= reach));
            expect(spots.length).toBeGreaterThan(1);
            expect(close).toEqual([]);
        }
    });

    it('keeps the places of a spot of level z + 1 that show at level z in one spot of level z', () => {
        for (let z = FIRST_LEVEL; z < 20; z++) {
            const below = levels[z - FIRST_LEVEL], shown = new Set(below.flatMap(spot => spot.places));
            expect(shown.size).toBe(below.reduce((sum, spot) => sum + spot.places.length, 0));
            for (const spot of levels[z + 1 - FIRST_LEVEL]) {
                const kept = spot.places.filter(place => shown.has(place));
                if (kept.length) expect(below.filter(other => kept.some(place => other.places.includes(place)))).toHaveLength(1);
            }
        }
    });

    it('shows the highest summit and the highest pass of a square first', () => {
        // The middle of the zoom-16 tile 16/34272/23013, so the places share each square of zoom 13 to 16.
        const lon = (34272.5 / 2 ** 16) * 360 - 180, lat = Math.atan(Math.sinh(Math.PI * (1 - 2 * 23013.5 / 2 ** 16))) * 180 / Math.PI;
        const shown = (places: TilePlace[], z: number) => groupTile(places, ['peak'])[z - FIRST_LEVEL].flatMap(spot => spot.places.map(p => p.id)).sort((a, b) => a - b);
        const peaks = [place(1, 'summit', lon, lat, 1000), place(2, 'summit', lon, lat), place(7, 'summit', lon, lat, 1000), place(9, 'pass', lon, lat, 900)];
        for (const z of [12, 15]) expect(shown(peaks, z)).toEqual([1, 9]);
        expect(shown(peaks, 16)).toEqual([1, 2, 7, 9]);
        // The next zoom-16 tile to the east shares its square of zoom 15 with the peaks above.
        const east = [...peaks, place(11, 'summit', lon + 360 / 2 ** 16, lat, 500)];
        expect(shown(east, 14)).toEqual([1, 9]);
        expect(shown(east, 15)).toEqual([1, 9, 11]);
        expect(shown([...peaks, place(5, 'summit', lon, lat, 2000)], 12)).toEqual([5, 9]);
    });

    it('keeps the key and the position of a spot when a peak appears', () => {
        const places = [place(1, 'summit', 8.3, 47.05, 100), place(2, 'summit', 8.3, 47.05, 900), place(5, 'bakery', 8.3, 47.05)];
        const grouped = groupTile(places, categoryIds), [spot] = grouped[0];
        expect(spot.places.map(p => p.id)).toEqual([2, 5]);
        for (const level of grouped) expect(level).toEqual([expect.objectContaining({ key: spot.key, lon: spot.lon, lat: spot.lat })]);
    });

    it('changes no spot of a tile when a neighbouring tile groups', () => {
        const first = town(200, 3), before = structuredClone(groupTile(first, categoryIds));
        // The places of the tile to the east, against the shared border.
        groupTile(town(200, 4, 8.4375).map(place => ({ ...place, lon: 8.4375 + (place.lon - 8.4375) / 20 })), categoryIds);
        expect(groupTile(first, categoryIds)).toEqual(before);
    });
});

describe('place marks', () => {
    const places = town(400, 7);
    const quiet: PlaceCategory[] = categoryIds.filter(category => category !== 'water' && category !== 'food');

    it('gives only the discs of the highlighted kind below level 16', () => {
        const discs = discMarks(places, 'water'), rows = rowMarks(places, quiet);
        for (let z = FIRST_LEVEL; z < 16; z++) {
            expect(rows[z - FIRST_LEVEL]).toEqual([]);
            expect(discs[z - FIRST_LEVEL].length).toBeGreaterThan(0);
            for (const mark of discs[z - FIRST_LEVEL]) {
                expect(mark).toMatchObject({ disc: true, icons: [{ category: 'water' }] });
                expect(mark.icons[0].places.every(p => p.category === 'water')).toBe(true);
            }
        }
    });

    it('gives a row one icon for each kind of its spot, in the order of the kinds', () => {
        const rows = rowMarks(places, quiet);
        for (let z = 16; z <= 20; z++) {
            const spots = groupTile(places, quiet)[z - FIRST_LEVEL];
            expect(rows[z - FIRST_LEVEL].map(mark => mark.key)).toEqual(spots.map(spot => `r${spot.key}`));
            for (const [index, mark] of rows[z - FIRST_LEVEL].entries()) {
                const kinds = mark.icons.map(icon => icon.category);
                expect(mark.disc).toBe(false);
                expect(kinds).toEqual(categoryIds.filter(category => spots[index].places.some(p => p.category === category)));
                expect(kinds.every(kind => quiet.includes(kind))).toBe(true);
                for (const icon of mark.icons) expect(icon.places.every(p => p.category === icon.category)).toBe(true);
            }
        }
        expect(rows[16 - FIRST_LEVEL].some(mark => mark.icons.length > 1)).toBe(true);
    });

    it('opens one place, lists up to eight, and zooms to more below level 17', () => {
        const some = (count: number) => places.slice(0, count);
        expect(opens(some(1), 12)).toBe('place');
        expect(opens(some(8), 12)).toBe('list');
        expect(opens(some(9), 16)).toBe('zoom');
        expect(opens(some(9), 17)).toBe('list');
    });
});
