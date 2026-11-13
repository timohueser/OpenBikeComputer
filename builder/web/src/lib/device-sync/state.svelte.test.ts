// @vitest-environment happy-dom

import { IDBFactory } from 'fake-indexeddb';
import { flushSync } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { deviceHolder } from '../device/session.svelte';
import { Library } from '../library/store';
import type { DeviceSession } from '../usb/session';
import { rememberCard, remembered } from './remembered.svelte';
import { deviceSync, startDeviceSync } from './state.svelte';
import { syncDevice, type SyncReport } from './sync';

vi.mock('./sync', () => ({ syncDevice: vi.fn() }));

const SERIAL = '9F3A00C1E2B47D10';
const session = (status: DeviceSession['status']) =>
    ({ status, client: status === 'ready' ? {} : null, info: { serialNumber: SERIAL }, store: null }) as unknown as DeviceSession;
const report = (tried: string[]): SyncReport => ({ rides: 0, marked: 0, sent: 0, removed: 0, waypointsLeft: 0, tried, problem: null });
const settle = () => new Promise(resolve => setTimeout(resolve, 20));

afterEach(() => vi.restoreAllMocks());

describe('startDeviceSync', () => {
    it('syncs again after a reconnection during a sync, and not for its own writes', async () => {
        vi.spyOn(deviceHolder, 'open').mockResolvedValue(null);
        const library = new Library(new IDBFactory());
        rememberCard(SERIAL, '8f2c41d96b074ea3b1559c207de83466');
        let release!: () => void;
        const signals: AbortSignal[] = [];
        vi.mocked(syncDevice).mockImplementation(async (_client, _serial, lib, signal) => {
            signals.push(signal);
            if (signals.length === 1) await new Promise<void>(resolve => release = resolve);
            // A route that the routing cannot calculate stays in `waiting`; the write must start no sync.
            await lib.changeState('waiting', () => ['offline']);
            return report(['offline']);
        });
        deviceHolder.session = session('ready');
        const stop = startDeviceSync(library);
        flushSync();
        await vi.waitFor(() => expect(signals).toHaveLength(1));

        // The cable goes out and in while the first sync waits.
        deviceHolder.session = session('idle');
        flushSync();
        expect(signals[0].aborted).toBe(true);
        deviceHolder.session = session('ready');
        flushSync();
        release();
        await vi.waitFor(() => expect(signals).toHaveLength(2));
        await settle();
        expect([signals.length, deviceSync.running]).toEqual([2, false]);

        // Only an id that no sync tried starts one.
        await library.changeState('waiting', ids => [...ids ?? [], 'new']);
        await vi.waitFor(() => expect(signals).toHaveLength(3));
        stop();
        deviceHolder.session = null;
        remembered.device = null;
    });
});
