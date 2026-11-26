// @vitest-environment happy-dom
import { flushSync } from 'svelte';
import { describe, expect, it } from 'vitest';
import { HoverState } from './hover.svelte';

describe('HoverState', () => {
    it('lights the line under the pointer over the focused row', () => {
        const hover = new HoverState();
        hover.focus('a');
        hover.point('b', 'map');
        expect(hover.id).toBe('b');
        hover.leave('map');
        expect(hover.id).toBe('a');
    });

    // A click on a row that moves the map ends with a move start on the map: the row stays lit.
    it('clears the pointer only from the side that it is on', () => {
        const hover = new HoverState();
        hover.point('a', 'list');
        hover.leave('map');
        expect(hover.id).toBe('a');
        hover.point('b', 'map');
        hover.leave('list');
        expect(hover.id).toBe('b');
    });

    it('runs no effect again for a pointer that stays on the same line', () => {
        const hover = new HoverState();
        let runs = 0;
        const stop = $effect.root(() => { $effect(() => { void hover.id; runs++; }); });
        flushSync();
        hover.point('a', 'list');
        flushSync();
        hover.point('a', 'list');
        hover.point('a', 'map');
        flushSync();
        expect(runs).toBe(2);
        stop();
    });

    it('takes the row under a still pointer after the list moves, but not under a pointer on the map', () => {
        const hover = new HoverState();
        hover.point('a', 'list');
        hover.lookup(() => 'b');
        expect(hover.id).toBe('b');
        hover.point('c', 'map');
        hover.lookup(() => 'd');
        expect(hover.id).toBe('c');
    });

    it('keeps the pointer value when the focus goes', () => {
        const hover = new HoverState();
        hover.focus('a');
        hover.point('b', 'list');
        hover.blur();
        expect(hover.id).toBe('b');
    });
});
