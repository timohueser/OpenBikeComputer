import { beforeEach, describe, expect, it } from 'vitest';
import { remembered } from '../../lib/device-sync/remembered.svelte';
import type { RouteRecord } from '../../lib/library/records';
import { sentKey } from '../../lib/library/summary';
import { deviceState } from './state';

const route: RouteRecord = { id: 'r', revision: 1, name: 'Lake loop', updatedAt: 0, summary: null, trip: { points: [] } };
const sent = { r: { device: 'card:7', key: sentKey(route) } };

beforeEach(() => {
    remembered.device = { serial: '00AB', storeId: 'card', at: null, map: null, mapObject: null, built: null, routes: 1, bytes: { map: 0, routes: 0, rides: 0, other: 0 }, came: null, skipped: [] };
});

describe('deviceState', () => {
    it('is on the device until the route is renamed after it was sent, and waits while it is in the list', () => {
        expect(deviceState(route, [], sent)).toBe('sent');
        expect(deviceState({ ...route, name: 'Lake loop, longer' }, [], sent)).toBe('changed');
        expect(deviceState(route, ['r'], sent)).toBe('waiting');
    });

    it('is not on the device for a route of another card, or with no remembered device', () => {
        expect(deviceState(route, [], { r: { device: 'other:7', key: sentKey(route) } })).toBeUndefined();
        remembered.device = null;
        expect(deviceState(route, [], sent)).toBeUndefined();
    });
});
