// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { allStay, type Stay } from '../../lib/planner/editor';
import StayChip from './StayChip.svelte';

let chip: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (chip) await unmount(chip); document.body.replaceChildren(); });

const trigger = () => document.querySelector<HTMLButtonElement>('.planner-chip')!;
const box = (label: string) => [...document.querySelectorAll('label')].find(row => row.textContent === label)!.querySelector('input')!;

describe('stay chip', () => {
    it('names one kind, the first kind and the count of the others, or any place for all five', () => {
        let value = $state<Stay[]>(['hut']);
        chip = mount(StayChip, { target: document.body, props: { get value() { return value; }, onChange: () => {} } });
        flushSync();
        expect(trigger().textContent).toBe('Huts');
        value = ['camp', 'hostel', 'hotel']; flushSync();
        expect(trigger().textContent).toBe('Campsites +2');
        value = allStay; flushSync();
        expect(trigger().textContent).toBe('Any place');
    });

    it('keeps the last checked kind', () => {
        let value = $state<Stay[]>(['camp', 'hut']);
        chip = mount(StayChip, { target: document.body, props: { get value() { return value; }, onChange: (kinds: Stay[]) => value = kinds } });
        flushSync();
        trigger().click(); flushSync();
        expect(box('Campsites').disabled).toBe(false);
        box('Huts').click(); flushSync();
        expect(value).toEqual(['camp']);
        expect(box('Campsites').disabled).toBe(true);
        expect(box('Huts').disabled).toBe(false);
    });

    it('closes on Escape and gives the focus back when the only kind is the first', async () => {
        chip = mount(StayChip, { target: document.body, props: { value: ['camp'], onChange: () => {} } });
        flushSync();
        trigger().click(); await tick();
        expect(document.activeElement).toBe(box('Huts'));
        document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); flushSync();
        expect(document.querySelector('label')).toBeNull();
        expect(document.activeElement).toBe(trigger());
    });
});
