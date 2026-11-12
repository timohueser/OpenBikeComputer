import { expect, it } from 'vitest';
import { heartRateZones, powerZones } from './zones';

const bounds = (zones: { from: number; to: number }[]) => zones.map(({ from, to }) => [from, to]);

it('gives the five heart rate zones of the device, with a value on an edge in the upper zone', () => {
    expect(bounds(heartRateZones(185))).toEqual([[0, 110], [111, 129], [130, 147], [148, 166], [167, Infinity]]);
    expect(heartRateZones(200).map(zone => zone.from)).toEqual([0, 120, 140, 160, 180]);
});

it('gives the five power zones of the device, where only the 55 % edge belongs to the upper zone', () => {
    expect(bounds(powerZones(250))).toEqual([[0, 137], [138, 187], [188, 225], [226, 262], [263, Infinity]]);
    expect(powerZones(200).map(zone => zone.from)).toEqual([0, 110, 151, 181, 211]);
    expect(powerZones(200).map(zone => zone.name)).toEqual(['Z1', 'Z2', 'Z3', 'Z4', 'Z5']);
});
