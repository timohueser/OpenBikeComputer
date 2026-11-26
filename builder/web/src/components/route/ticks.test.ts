import { describe, expect, it } from 'vitest';
import { distanceTicks, fitTicks } from './ticks';

describe('the ticks of a stretch of a line', () => {
    it('are at round distances from the start of the line, not of the stretch', () => {
        const ticks = distanceTicks(14_400, 21_300, 730);
        expect(ticks.map(tick => tick.label)).toEqual(['km 15', 'km 16', 'km 17', 'km 18', 'km 19', 'km 20', 'km 21']);
        expect(ticks[0].at).toBeCloseTo(600 / 6900);
    });

    it('have steps under one kilometre for a short stretch only when the ticks are fine', () => {
        expect(distanceTicks(16_500, 17_900, 730, true).map(tick => tick.label)).toEqual(['km 16.6', 'km 16.8', 'km 17.0', 'km 17.2', 'km 17.4', 'km 17.6', 'km 17.8']);
        expect(distanceTicks(16_500, 17_900, 730).map(tick => tick.label)).toEqual(['km 17']);
    });
});

describe('fitTicks', () => {
    it('leaves out a tick that would touch a label of `taken`', () => {
        const wanted = [{ at: 0, label: 'Start' }, { at: 0.52, label: 'km 20' }, { at: 0.8, label: 'km 30' }];
        expect(fitTicks(wanted, 400, [{ at: 0.5, label: '3' }]).map(tick => tick.label)).toEqual(['Start', 'km 30']);
    });
});
