import { describe, expect, it } from 'vitest';
import type { Bounds } from './config';
import type { Coordinate } from './geo';
import { nightView } from './view-change';

const view: Bounds = [8, 47, 9, 48];
const far: Coordinate[] = [[12, 50]];

describe('night view', () => {
    it('stays when the day end and three listed towns are in view, a town by any of its places, or all towns when there are fewer', () => {
        const straddling: Coordinate[] = [[8.95, 47.5], [9.05, 47.5]];
        expect(nightView(view, [8.5, 47.5], [[[8.4, 47.4]], straddling, far, [[8.6, 47.6]]])).toEqual({ action: 'stay' });
        expect(nightView(view, [8.5, 47.5], [[[8.4, 47.4]]])).toEqual({ action: 'stay' });
        expect(nightView(view, [8.5, 47.5], [])).toEqual({ action: 'stay' });
    });

    it('pans to the day end and the three nearest towns when their box takes at most 80 % of the view', () => {
        // The third town is out of view, and the far fourth town does not widen the box.
        expect(nightView(view, [8.5, 47.5], [[[8.4, 47.4]], [[8.6, 47.6]], [[9.1, 47.5], [9.15, 47.55]], far]))
            .toEqual({ action: 'pan', box: [8.4, 47.4, 9.15, 47.6] });
        // Three towns in view do not keep a day end out of view.
        expect(nightView(view, [9.2, 47.5], [[[8.9, 47.5]], [[8.8, 47.45]], [[8.7, 47.5]]])).toEqual({ action: 'pan', box: [8.7, 47.45, 9.2, 47.5] });
    });

    it('fits the day end and the nearest towns when their box is too high or too wide for a pan', () => {
        expect(nightView(view, [8.5, 46], [[[8.5, 45.5]], [[8.6, 46.4]]])).toEqual({ action: 'fit', box: [8.5, 45.5, 8.6, 46.4] });
        expect(nightView(view, [8.5, 47.5], [[[7.9, 47.5]], [[8.9, 47.5]]])).toEqual({ action: 'fit', box: [7.9, 47.5, 8.9, 47.5] });
    });
});
