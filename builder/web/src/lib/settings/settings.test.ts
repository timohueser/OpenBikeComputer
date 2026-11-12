// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { distanceIn, formatDistance, formatHeight, formatSpeed, heightIn, heightUnit } from './format';
import { readSettings, settings, takeDeviceValues, updateSettings } from './settings.svelte';

const stored = new Map<string, string>();
beforeEach(() => {
    stored.clear();
    vi.stubGlobal('localStorage', { getItem: (key: string) => stored.get(key) ?? null, setItem: (key: string, value: string) => stored.set(key, value) });
    updateSettings(readSettings(null));
});
/** A write of another tab. */
const otherTab = (value: object) => {
    stored.set('obc-settings', JSON.stringify({ ...JSON.parse(stored.get('obc-settings')!), ...value }));
    window.dispatchEvent(new StorageEvent('storage', { key: 'obc-settings' }));
};

describe('readSettings', () => {
    it('keeps the valid fields, takes the defaults for the others, and keeps rider values inside the device ranges', () => {
        expect(readSettings('nonsense')).toEqual({ units: 'km', maxHr: null, ftp: null });
        expect(readSettings({ units: 'yards', pace: 'fast', maxHr: { value: '180', at: 1, device: true }, ftp: { value: 250, at: 1 } }))
            .toEqual({ units: 'km', maxHr: null, ftp: null });
        expect(readSettings({ units: 'mi', maxHr: { value: 250, at: 5, device: false }, ftp: { value: 10, at: 6, device: true } }))
            .toEqual({ units: 'mi', maxHr: { value: 220, at: 5, device: false }, ftp: { value: 50, at: 6, device: true } });
    });
});

describe('updateSettings', () => {
    it('keeps the newer write of another tab, and takes it into this tab', () => {
        stored.set('obc-settings', JSON.stringify({ ...readSettings(null), units: 'mi' }));
        updateSettings({ maxHr: { value: 180, at: 1, device: false } });
        expect(JSON.parse(stored.get('obc-settings')!)).toMatchObject({ units: 'mi', maxHr: { value: 180 } });
        otherTab({ units: 'km' });
        expect(settings).toMatchObject({ units: 'km', maxHr: { value: 180 } });
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

    it('rounds before it chooses the unit and the digits, and shows a dash for a value that is not a number', () => {
        expect([formatDistance(99_960), formatDistance(999.6), formatDistance(-0.2)]).toEqual(['100 km', '1.0 km', '0 m']);
        expect([formatDistance(NaN), formatHeight(Infinity), formatSpeed(NaN)]).toEqual(['–', '–', '–']);
        updateSettings({ units: 'mi' });
        expect([formatDistance(40), formatDistance(0), formatDistance(160_900)]).toEqual(['0.1 mi', '0.0 mi', '100 mi']);
    });

    it('writes distances, heights and speeds in miles', () => {
        updateSettings({ units: 'mi' });
        expect([formatDistance(400), formatDistance(12_231), formatDistance(200_000)]).toEqual(['0.2 mi', '7.6 mi', '124 mi']);
        expect([formatHeight(1240), heightUnit()]).toEqual(['4,068 ft', 'ft']);
        expect(distanceIn(1609.344)).toBe(1);
        expect(formatSpeed(19 / 3.6)).toBe('11.8 mph');
    });
});
