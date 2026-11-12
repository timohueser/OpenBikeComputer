import { afterEach, describe, expect, it, vi } from 'vitest';
import { initCore } from '../core/bridge';
import { openDeviceSession } from '../usb/session.svelte';
import { deviceHolder } from './session.svelte';

vi.mock('../core/bridge', () => ({ initCore: vi.fn(async () => ({})) }));
vi.mock('../usb/session.svelte', () => ({ openDeviceSession: vi.fn(async () => ({ status: 'idle' })) }));

afterEach(() => vi.unstubAllGlobals());

describe('deviceHolder', () => {
    it('loads the shared core before it opens the session, so a sync on a page with no core can run', async () => {
        vi.stubGlobal('navigator', { usb: {} });
        await deviceHolder.open();
        expect(initCore).toHaveBeenCalledOnce();
        expect(openDeviceSession).toHaveBeenCalledOnce();
        expect(deviceHolder.session).toEqual({ status: 'idle' });
    });
});
