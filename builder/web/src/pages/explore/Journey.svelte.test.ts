// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it } from 'vitest';
import Journey from './Journey.svelte';
import { defaultQuery, type Query, type Shape } from './search';

let journey: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (journey) await unmount(journey); document.body.replaceChildren(); });

/** The change that a click on Lifts gives, with lifts on and `shapes` accepted. */
function liftsOff(shapes: Shape[]): Partial<Query> {
    const changes: Partial<Query>[] = [];
    journey = mount(Journey, { target: document.body, props: {
        query: { ...defaultQuery('2026-07-01'), lift: 'helps', shapes }, cap: 45, place: () => undefined, picking: false, onpicking: () => {}, onchange: (change: Partial<Query>) => changes.push(change),
    } });
    flushSync();
    [...document.querySelectorAll('button')].find(button => button.textContent?.includes('Lifts'))!.click();
    return changes[0];
}

it('turns lifts off and keeps the shapes when loop or there and back is on', () => {
    expect(liftsOff(['back', 'liftUp'])).toEqual({ lift: undefined, shapes: ['back', 'liftUp'] });
});

it('turns loop and there and back on with lifts off when no shape of them is on', () => {
    expect(liftsOff(['liftUp'])).toEqual({ lift: undefined, shapes: ['loop', 'back', 'liftUp'] });
});
