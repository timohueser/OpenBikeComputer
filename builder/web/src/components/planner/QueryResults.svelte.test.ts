// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import QueryResults from './QueryResults.svelte';
import { dayColor } from '../../lib/planner/day-colors';
import type { SearchPlace } from '../../lib/planner/search/types';

const days = [{ number: 1, from: 0, to: 60 }, { number: 3, from: 60, to: 120 }];
const place = (source: string, kind: string, position?: SearchPlace['position']): SearchPlace =>
    ({ source, name: source, kind, lon: 8, lat: 47, city: 'Eich', region: '', distance: 2.34, position, precision: 'place' });
let component: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (component) await unmount(component); component = undefined; document.body.replaceChildren(); });
/** Each row as its second line and its distance column. */
async function show(results: SearchPlace[], props: Record<string, unknown> = {}) {
    if (component) await unmount(component);
    document.body.replaceChildren();
    component = mount(QueryResults, { target: document.body, props: { attribution: 'OSM', days, theme: 'light', selectedId: null,
        state: { loading: false, error: '', answer: { type: 'places', request: { type: 'places', what: ['hotel'] }, results, area: 'Along the route' } },
        onSelect() {}, onApply() {}, onMore() {}, onRetry() {}, onStretch() {}, ...props } });
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
    it('gives each place the action of "Find", amber on the first when the rider chose it, and says what an action changed with "Undo"', async () => {
        const onAct = vi.fn(), onUndo = vi.fn();
        const actions = () => [...document.querySelectorAll<HTMLButtonElement>('.result .act')].map(button => [button.textContent, button.classList.contains('primary')]);
        await show([place('a', 'hotel'), place('b', 'hotel')], { action: { button: 'Add as a stop', chosen: false } });
        expect(actions()).toEqual([['Add as a stop', false], ['Add as a stop', false]]);
        await show([place('a', 'hotel'), place('b', 'hotel')], { action: { button: 'Route here', chosen: true }, done: 'The route now ends at a', onAct, onUndo });
        expect(actions()).toEqual([['Route here', true], ['Route here', false]]);
        document.querySelectorAll<HTMLButtonElement>('.result .act')[1].click();
        expect(onAct).toHaveBeenCalledWith(expect.objectContaining({ source: 'b' }));
        expect(document.querySelector('.done')?.textContent).toBe('The route now ends at aUndo');
        document.querySelector<HTMLButtonElement>('.done button')!.click();
        expect(onUndo).toHaveBeenCalledOnce();
    });
    it('shows a change of the plan that the slots cannot hold as one action row with "Apply"', async () => {
        const onApply = vi.fn();
        const changes = [{ op: 'route' as const, points: [{ coordinate: [8, 47] as [number, number], label: 'Freiburg' }, { coordinate: [9, 47] as [number, number], label: 'Konstanz', detail: 'Altstadt' }] }];
        component = mount(QueryResults, { target: document.body, props: { attribution: 'OSM', days, theme: 'light', selectedId: null, onApply,
            state: { loading: false, error: '', answer: { type: 'change', request: { type: 'route', from: { name: 'Freiburg' }, to: { name: 'Konstanz' } }, description: 'Route from Freiburg to Konstanz', changes } },
            onSelect() {}, onMore() {}, onRetry() {}, onStretch() {} } });
        await tick();
        expect([...document.querySelectorAll('.action-row :is(strong, small, button)')].map(node => node.textContent)).toEqual(['Route from Freiburg to Konstanz', 'Freiburg · Konstanz, Altstadt', 'Apply']);
        document.querySelector<HTMLButtonElement>('.action-row button')!.click();
        expect(onApply).toHaveBeenCalledOnce();
    });
    it('shows the area of an empty list without a count', async () => {
        await show([]);
        expect(document.querySelector('.result-heading')?.textContent).toBe('Along the route');
    });
    it('gives a stretch of the list as its km from the answer, also where the route passes the same road twice', async () => {
        const onStretch = vi.fn();
        // Out and back: the climb at km 40–45 lies on the road of km 15–20.
        const line: [number, number][] = [[8.02, 47], [8.01, 47]];
        component = mount(QueryResults, { target: document.body, props: { attribution: 'OSM', days, theme: 'light', selectedId: null,
            state: { loading: false, error: '', answer: { type: 'stretches', request: { type: 'stretches', what: 'climb' }, stretches: [{ from: 40, to: 45, label: 'Climb', coordinates: line }] } },
            onSelect() {}, onApply() {}, onMore() {}, onRetry() {}, onStretch } });
        await tick();
        document.querySelector<HTMLButtonElement>('.stretch')!.click();
        expect(onStretch).toHaveBeenCalledWith([40, 45]);
    });
});
