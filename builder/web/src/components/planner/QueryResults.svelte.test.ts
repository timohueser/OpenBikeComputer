// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import QueryResults from './QueryResults.svelte';
import { dayColor } from '../../lib/planner/day-colors';
import type { SearchPlace } from '../../lib/planner/search/types';

const days = [{ number: 1, from: 0, to: 60 }, { number: 3, from: 60, to: 120 }];
const place = (source: string, kind: string, position?: SearchPlace['position']): SearchPlace =>
    ({ source, name: source, kind, lon: 8, lat: 47, city: 'Eich', region: '', distance: 2.34, position, precision: 'place' });
let component: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (component) await unmount(component); component = undefined; document.body.replaceChildren(); });
/** Each row as its second line and its distance column. */
async function show(results: SearchPlace[]) {
    if (component) await unmount(component);
    document.body.replaceChildren();
    component = mount(QueryResults, { target: document.body, props: { attribution: 'OSM', days, theme: 'light', selectedId: null,
        state: { loading: false, error: '', answer: { type: 'places', request: { type: 'places', what: ['hotel'] }, results, area: 'Along the route' } },
        onSelect() {}, onApply() {}, onMore() {}, onRetry() {}, onStretch() {} } });
    await tick();
    return [...document.querySelectorAll('.place-row')].map(row => [...row.querySelectorAll('small, .distance b, .distance span')].map(line => line.textContent));
}
describe('search results', () => {
    it('groups results along a trip by riding day and shows their km marks', async () => {
        const rows = await show([place('a', 'hotel', { along: 5.84, distance: 0.21 }), place('b', 'hotel', { along: 60, distance: 0.04 }), place('c', 'hotel', { along: 61, distance: 0 })]);
        const heads = [...document.querySelectorAll<HTMLElement>('.day-head')];
        expect(heads.map(head => head.textContent)).toEqual(['Day 1', 'Day 3']);
        expect(heads[1].nextElementSibling?.querySelector('strong')?.textContent).toBe('c');
        expect(heads[1].querySelector('span')!.style.background).toBe(dayColor(2, 'light'));
        expect(rows).toEqual([['Eich', 'km 5.8', '0.2 km off'], ['Eich', 'km 60.0', 'on route'], ['Eich', 'km 61.0', 'on route']]);
    });
    it('shows no day headings for results ranked out of route order', async () => {
        await show([place('a', 'campsite', { along: 70, distance: 0.1 }), place('b', 'campsite', { along: 50, distance: 0.1 }), place('c', 'campsite', { along: 80, distance: 0.1 })]);
        expect(document.querySelector('.day-head')).toBeNull();
    });
    it('shows the kind only when the results mix kinds, and the distance without a route', async () => {
        expect(await show([place('a', 'hotel'), place('b', 'hotel')])).toEqual([['Eich', '2.3 km'], ['Eich', '2.3 km']]);
        expect(document.querySelector('.day-head')).toBeNull();
        expect(await show([place('a', 'hotel'), place('b', 'campsite')])).toEqual([['hotel · Eich', '2.3 km'], ['campsite · Eich', '2.3 km']]);
    });
});
