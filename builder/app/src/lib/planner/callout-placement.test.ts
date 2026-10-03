import { describe, expect, it } from 'vitest';
import { calloutBox, placeCallout } from './callout-placement';

// A 1000 × 600 map with 16 px margins, the controls column on the right and a date bar at the bottom.
const free = { left: 16, top: 16, right: 932, bottom: 480 };
const size: [number, number] = [340, 200];

describe('callout placement', () => {
    it('draws boxes where MapLibre puts them', () => {
        expect(calloutBox('bottom', [500, 300], size, 20)).toEqual({ left: 330, top: 70, right: 670, bottom: 280 });
        expect(calloutBox('top-left', [500, 300], size, 20)).toEqual({ left: 514, top: 314, right: 854, bottom: 524 });
        expect(calloutBox('right', [500, 300], size, 20)).toEqual({ left: 130, top: 200, right: 480, bottom: 400 });
    });

    it('opens above the point when it fits', () => {
        expect(placeCallout([500, 300], size, free, 20)).toEqual({ anchor: 'bottom', pan: [0, 0] });
    });

    it('opens below a point near the top', () => {
        expect(placeCallout([500, 60], size, free, 20).anchor).toBe('top');
    });

    it('keeps clear of the controls and the left edge with a corner', () => {
        expect(placeCallout([880, 300], size, free, 20).anchor).toBe('bottom-right');
        expect(placeCallout([40, 300], size, free, 20).anchor).toBe('bottom-left');
    });

    it('opens beside the point when neither above nor below fits', () => {
        const tall: [number, number] = [340, 330];
        expect(placeCallout([300, 250], tall, free, 20).anchor).toBe('left');
        expect(placeCallout([800, 250], tall, free, 20).anchor).toBe('right');
    });

    it('pans the least distance when nothing fits', () => {
        // A narrow map: no room beside the point, and the date bar below it.
        const narrow = { left: 16, top: 16, right: 400, bottom: 300 };
        expect(placeCallout([200, 120], [340, 180], narrow, 20)).toEqual({ anchor: 'top', pan: [0, 30] });
    });
});
