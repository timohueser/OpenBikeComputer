import { describe, expect, it } from 'vitest';
import { FIRST_LEVEL, groupTile, type Badge, type TilePlace } from './place-groups';
import { tileColumn, tileRow } from './place-index';
import { categoryIds, poiKinds } from './poi-kinds';

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
const pixels = (badge: Badge, z: number) => [tileColumn(badge.lon, z) * 512, tileRow(badge.lat, z) * 512];

describe('place groups', () => {
    it('gives the same badges for the same places in another order', () => {
        const places = town(400, 7);
        expect(groupTile([...places].reverse(), categoryIds)).toEqual(groupTile(places, categoryIds));
    });

    it('keeps no two badges of a level inside the reach of each other', () => {
        for (let z = FIRST_LEVEL; z <= 20; z++) {
            const reach = z >= 16 ? 32 : 64, badges = levels[z - FIRST_LEVEL].map(badge => pixels(badge, z));
            const close = badges.flatMap((a, i) => badges.slice(i + 1).filter(b => Math.abs(a[0] - b[0]) <= reach && Math.abs(a[1] - b[1]) <= reach));
            expect(badges.length).toBeGreaterThan(1);
            expect(close).toEqual([]);
        }
    });

    it('puts each badge of level z + 1 in its parent of level z', () => {
        for (let z = FIRST_LEVEL; z < 20; z++) {
            const below = levels[z - FIRST_LEVEL], shown = new Set(below.flatMap(badge => badge.places));
            expect(shown.size).toBe(below.reduce((sum, badge) => sum + badge.places.length, 0));
            for (const badge of levels[z + 1 - FIRST_LEVEL]) {
                const kept = badge.places.filter(place => shown.has(place));
                const parent = below.find(other => other.key === badge.parent);
                if (kept.length) expect(parent?.places).toEqual(expect.arrayContaining(kept));
                else expect(badge.parent).toBeUndefined();
            }
        }
        expect(levels[0].some(badge => badge.parent !== undefined)).toBe(false);
    });

    it('shows the highest summit and the highest pass of a square first', () => {
        // The middle of the zoom-16 tile 16/34272/23013, so the places share each square of zoom 13 to 16.
        const lon = (34272.5 / 2 ** 16) * 360 - 180, lat = Math.atan(Math.sinh(Math.PI * (1 - 2 * 23013.5 / 2 ** 16))) * 180 / Math.PI;
        const shown = (places: TilePlace[], z: number) => groupTile(places, ['peak'])[z - FIRST_LEVEL].flatMap(badge => badge.places.map(p => p.id)).sort((a, b) => a - b);
        const peaks = [place(1, 'summit', lon, lat, 1000), place(2, 'summit', lon, lat), place(7, 'summit', lon, lat, 1000), place(9, 'pass', lon, lat, 900)];
        for (const z of [12, 15]) expect(shown(peaks, z)).toEqual([1, 9]);
        expect(shown(peaks, 16)).toEqual([1, 2, 7, 9]);
        // The next zoom-16 tile to the east shares its square of zoom 15 with the peaks above.
        const east = [...peaks, place(11, 'summit', lon + 360 / 2 ** 16, lat, 500)];
        expect(shown(east, 14)).toEqual([1, 9]);
        expect(shown(east, 15)).toEqual([1, 9, 11]);
        expect(shown([...peaks, place(5, 'summit', lon, lat, 2000)], 12)).toEqual([5, 9]);
    });

    it('keeps the key and the position of a badge when a peak appears', () => {
        const spot = [place(1, 'summit', 8.3, 47.05, 100), place(2, 'summit', 8.3, 47.05, 900), place(5, 'bakery', 8.3, 47.05)];
        const grouped = groupTile(spot, categoryIds), [badge] = grouped[0];
        expect(badge.places.map(p => p.id)).toEqual([2, 5]);
        for (const level of grouped) expect(level).toEqual([expect.objectContaining({ key: badge.key, lon: badge.lon, lat: badge.lat })]);
    });

    it('changes no badge of a tile when a neighbouring tile groups', () => {
        const first = town(200, 3), before = structuredClone(groupTile(first, categoryIds));
        // The places of the tile to the east, against the shared border.
        groupTile(town(200, 4, 8.4375).map(place => ({ ...place, lon: 8.4375 + (place.lon - 8.4375) / 20 })), categoryIds);
        expect(groupTile(first, categoryIds)).toEqual(before);
    });
});
