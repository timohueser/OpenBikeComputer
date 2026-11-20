// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import QueryAlong from './QueryAlong.svelte';
import type { Along } from '../../lib/planner/search/types';

let component: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (component) await unmount(component); component = undefined; document.body.replaceChildren(); });
/** The interval after the rider types `n` into the field `label` of the interval `value`. */
async function enter(value: Along, label: 'From' | 'To', n: number) {
    const onChange = vi.fn();
    component = mount(QueryAlong, { target: document.body, props: { value, onChange } });
    await tick();
    const input = [...document.querySelectorAll('label')].find(l => l.textContent?.startsWith(label))!.querySelector('input')!;
    input.value = String(n); input.dispatchEvent(new Event('change', { bubbles: true }));
    return onChange.mock.lastCall![0] as Along;
}
const km = (value: number) => ({ value, unit: 'km' });

it('moves the other end of the interval along when one end passes it, so To stays after From', async () => {
    expect(await enter({ ref: 'km', from: km(0), to: km(10) }, 'From', 14.3)).toEqual({ ref: 'km', from: km(14.3), to: km(24.3) });
    await unmount(component!);
    expect(await enter({ ref: 'km', from: km(14), to: km(20) }, 'To', 4)).toEqual({ ref: 'km', from: km(0), to: km(4) });
    await unmount(component!);
    expect(await enter({ ref: 'km', from: km(0), to: km(10) }, 'From', 4)).toEqual({ ref: 'km', from: km(4), to: km(10) });
});
