import { describe, expect, it } from 'vitest';
import type { Bounds } from './config';
import { nightView } from './view-change';

const view: Bounds = [8, 47, 9, 48];

describe('night view', () => {
    it('stays when the day end and every stop are in view with room, or when there is no stop', () => {
        expect(nightView(view, [8.5, 47.5], [[[8.2, 47.3]], [[8.9, 47.8]]])).toEqual({ action: 'stay' });
        expect(nightView(view, [8.5, 47.5], [])).toEqual({ action: 'stay' });
    });

    it('fits the day end and the stops when they are small in the view', () => {
        expect(nightView(view, [8.5, 47.5], [[[8.45, 47.45]], [[8.6, 47.6]]])).toEqual({ action: 'fit', box: [8.45, 47.45, 8.6, 47.6] });
    });

    it('fits the day end and the stops when a place of a stop, or the day end, is out of view', () => {
        expect(nightView(view, [8.5, 47.5], [[[8.2, 47.4]], [[8.95, 47.5], [9.05, 47.5]]])).toEqual({ action: 'fit', box: [8.2, 47.4, 9.05, 47.5] });
        expect(nightView(view, [9.2, 47.5], [[[8.3, 47.5]], [[8.8, 47.45]]])).toEqual({ action: 'fit', box: [8.3, 47.45, 9.2, 47.5] });
    });
});
