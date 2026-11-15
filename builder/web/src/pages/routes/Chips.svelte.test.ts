// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import Chips from './Chips.svelte';
import type { Chips as ChipValues } from './filter';

let chips: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (chips) await unmount(chips); document.body.replaceChildren(); });

describe('chips', () => {
    it('keeps the menu open and the chip in its place while the slider changes, and reads only its values', () => {
        let current = $state<ChipValues>({ bike: 'road' });
        const now = () => current;
        const asked = new Set<string>();
        chips = mount(Chips, { target: document.body, props: {
            get chips() { return current; }, set chips(next: ChipValues) { current = next; },
            keys: ['bike', 'length', 'climb'], values: key => { asked.add(key); return [10, 20, 30]; },
        } });
        flushSync();
        const summaries = () => [...document.querySelectorAll('summary')].map(summary => summary.textContent!.replaceAll('\u00a0', ' '));
        expect(summaries()).toEqual(['Road bike ▾', '+ Length', '+ Climb']);

        const climb = document.querySelectorAll('details')[2];
        climb.open = true;
        climb.dispatchEvent(new Event('toggle'));
        flushSync();
        const to = document.querySelector<HTMLInputElement>('input[aria-label="Climb, to"]')!;
        to.value = '20';
        to.dispatchEvent(new Event('input', { bubbles: true }));
        flushSync();
        expect(now().climb).toEqual([0, 20]);
        expect(summaries()).toEqual(['Road bike ▾', '+ Length', 'Under 20 m ▾']);
        expect(document.querySelector('details.open')).toBe(climb);
        // Only the open slider reads its values.
        expect([...asked]).toEqual(['climb']);

        climb.open = false;
        climb.dispatchEvent(new Event('toggle'));
        flushSync();
        expect(summaries()).toEqual(['Road bike ▾', 'Under 20 m ▾', '+ Length']);
    });
});
