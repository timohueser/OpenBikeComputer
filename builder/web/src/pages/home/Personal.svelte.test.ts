// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import type { RideRecord } from '../../lib/library/records';
import type { Library } from '../../lib/library/store';
import Personal from './Personal.svelte';
import { homeText } from './personal';

let page: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (page) await unmount(page); document.body.replaceChildren(); });

const now = new Date(2026, 10, 13, 18);
const ride = (key: string, day: number, lengthM: number, bike = 'gravel') =>
    ({ key, name: key, summary: { start: new Date(2026, 5, day).getTime() / 1000, lengthM, climbM: 0, movingS: 0, bike } }) as RideRecord;
/** The label, each large number with the text under it, and the line of the count. */
function year(rides: RideRecord[]) {
    page = mount(Personal, { target: document.body, props: { library: {} as Library, text: homeText([], rides, null, now) } });
    flushSync();
    return [...document.querySelectorAll('.over, h1 span, h1.total, .numbers')].map(node => node.textContent!.replace(/\s+/g, ' ').trim());
}

describe('the text of the year', () => {
    const bike = [ride('one', 1, 62_000), ride('two', 2, 62_000)], walk = ride('walk', 3, 14_000, 'hiking');

    it('shows the lengths by bike and on foot side by side, each with its word and its count, and never adds them', () => {
        expect(year([...bike, walk])).toEqual(['This year', '124 kmby bike · 2 rides', '14.0 kmon foot · 1 hike']);
    });

    it('shows one large number for a year with only rides by bike, and for one with only hikes', async () => {
        expect(year(bike)).toEqual(['This year, by bike', '124 km', '2 rides']);
        await unmount(page!);
        expect(year([walk])).toEqual(['This year, on foot', '14.0 km', '1 hike']);
    });
});
