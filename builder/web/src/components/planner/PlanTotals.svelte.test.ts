// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import PlanTotals from './PlanTotals.svelte';

let totals: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (totals) await unmount(totals); document.body.replaceChildren(); });

const line = () => [...document.querySelectorAll('dd')].map(value => value.textContent).join(' · ');

describe('plan totals', () => {
    it('shows the totals as one line, and a dash for a total that is not known', async () => {
        totals = mount(PlanTotals, { target: document.body, props: { distance: 138.31, ascent: 4128, descent: 2351, hours: 10.13 } });
        flushSync();
        expect(line()).toBe('138.3 km · ↑ 4128 m · ↓ 2351 m · 10 h 8 m');
        expect([...document.querySelectorAll('dt')].map(label => label.textContent)).toEqual(['Distance', 'Ascent', 'Descent', 'Riding time']);
        await unmount(totals);

        totals = mount(PlanTotals, { target: document.body, props: { distance: 3, ascent: null, descent: null, hours: null, walking: true } });
        flushSync();
        expect(line()).toBe('3.0 km · ↑ — m · ↓ — m · —');
        expect(document.querySelector('dl')?.textContent).toContain('Walking time');
    });
});
