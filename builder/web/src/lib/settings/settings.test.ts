import { beforeEach, describe, expect, it } from 'vitest';
import { distanceIn, formatDistance, formatHeight, formatSpeed, heightIn, heightUnit, plannedSeconds } from './format';
import { readSettings, settings, takeDeviceValues, updateSettings } from './settings.svelte';

beforeEach(() => updateSettings(readSettings(null)));

describe('readSettings', () => {
    it('keeps the valid fields, takes the defaults for the others, and keeps rider values inside the device ranges', () => {
        expect(readSettings('nonsense')).toEqual({ units: 'km', pace: 'moderate', maxHr: null, ftp: null });
        expect(readSettings({ units: 'yards', pace: 'toString', maxHr: { value: '180', at: 1, device: true }, ftp: { value: 250, at: 1 } }))
            .toEqual({ units: 'km', pace: 'moderate', maxHr: null, ftp: null });
        expect(readSettings({ units: 'mi', pace: 'fast', maxHr: { value: 250, at: 5, device: false }, ftp: { value: 10, at: 6, device: true } }))
            .toEqual({ units: 'mi', pace: 'fast', maxHr: { value: 220, at: 5, device: false }, ftp: { value: 50, at: 6, device: true } });
    });
});

describe('takeDeviceValues', () => {
    it('replaces a missing value and an older one, and keeps a value newer than the ride', () => {
        updateSettings({ maxHr: { value: 185, at: 2000, device: false } });
        takeDeviceValues(1000, 190, 240);
        expect(settings.maxHr).toEqual({ value: 185, at: 2000, device: false });
        expect(settings.ftp).toEqual({ value: 240, at: 1000, device: true });
        takeDeviceValues(3000, 178, null);
        expect(settings.maxHr).toEqual({ value: 178, at: 3000, device: true });
        expect(settings.ftp).toEqual({ value: 240, at: 1000, device: true });
    });
});

describe('format', () => {
    it('writes distances, heights and speeds in kilometres', () => {
        expect([formatDistance(400), formatDistance(12_340), formatDistance(123_400), formatDistance(1_234_000)]).toEqual(['400 m', '12.3 km', '123 km', '1,234 km']);
        expect([formatHeight(1240), heightUnit(), heightIn(1240), distanceIn(12_340)]).toEqual(['1,240 m', 'm', 1240, 12.34]);
        expect(formatSpeed(19 / 3.6)).toBe('19.0 km/h');
    });

    it('writes distances, heights and speeds in miles', () => {
        updateSettings({ units: 'mi' });
        expect([formatDistance(400), formatDistance(12_231), formatDistance(200_000)]).toEqual(['0.2 mi', '7.6 mi', '124 mi']);
        expect([formatHeight(1240), heightUnit()]).toEqual(['4,068 ft', 'ft']);
        expect(distanceIn(1609.344)).toBe(1);
        expect(formatSpeed(19 / 3.6)).toBe('11.8 mph');
    });

    it('plans a ride at the pace of the rider, and leaves a time on foot as it is', () => {
        updateSettings({ pace: 'fast' });
        expect(plannedSeconds(2700, 'gravel')).toBe(1900);
        expect(plannedSeconds(2700, 'hiking')).toBe(2700);
    });
});
