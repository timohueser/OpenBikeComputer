/**
 * A device on a pretend cable: `planner.html?fake-device`, on the dev server and in the build of the browser tests. The wasm
 * test device of the sync tests holds a map, a route and two rides of today. The chooser of the Connect button plugs it in.
 * `window.deviceTest`: `plug(unformatted)` and `unplug()` move the cable, `hold()` stops each sync at its start until `release()`, and
 * `offer(version)` publishes a pretend software version.
 */
import { initCore } from '../../src/lib/core/bridge';
import { deviceHolder } from '../../src/lib/device/session.svelte';
import { firmwareCheck } from '../../src/lib/firmware/check.svelte';
import { flatDevice, initFlatDevice } from '../../src/lib/usb/flat-device';
import { encodeRideObject, type RideObject } from '../../src/lib/usb/objects';
import { ObjectKind } from '../../src/lib/usb/protocol';
import type { DeviceState, DeviceWatcher } from '../../src/lib/usb/session';
import { WatchedDeviceSession } from '../../src/lib/usb/session.svelte';

function ride(startTime: number, lon: number): RideObject {
    return {
        version: 6, isDemo: false, name: '', startTime, distanceM: 0, movingTimeS: 0, avgSpeedCms: 0, climbM: 0, descentM: 0,
        avgHr: null, maxHr: null, avgCadence: null, avgPower: null, maxPower: null, energyKj: null, bikeType: 1, trip: null,
        effortLimits: { maxHrBpm: null, ftpW: null },
        points: Array.from({ length: 60 }, (_, i) => ({ tMs: i * 20_000, latMicrodegrees: 47_000_000 + i * 300,
            lonMicrodegrees: lon + i * 200, elevationM: 400 + i, segmentStart: i === 0, hrBpm: null, cadenceRpm: null, powerW: null })),
    };
}

export async function fakeDevice(): Promise<void> {
    await Promise.all([initFlatDevice(), initCore()]);
    const rig = flatDevice({ storeId: '0f5e9a7c2b3d4e1f8a6b5c4d3e2f1a0b', deviceInfo: {
        firmwareRevision: '0.9.2', hardwareRevision: 'obc-lm20-r1', serialNumber: '9F3A00C1E2B44F2A' } });
    const now = Math.floor(Date.now() / 1000);
    rig.device.seed({ kind: ObjectKind.MapShard, displayName: 'Baden-Württemberg', bytes: new Uint8Array(40_000) });
    rig.device.seed({ kind: ObjectKind.Route, displayName: 'Lake loop', bytes: new Uint8Array(1_200) });
    rig.device.seed({ kind: ObjectKind.Ride, displayName: 'ride', bytes: encodeRideObject(ride(now - 7200, 8_000_000)) });
    rig.device.seed({ kind: ObjectKind.Ride, displayName: 'ride', bytes: encodeRideObject(ride(now - 3600, 8_050_000)) });

    const idle: DeviceState = { status: 'idle', client: null, store: null, info: null, error: null };
    let state = idle;
    const listeners = new Set<(state: DeviceState) => void>();
    const publish = (next: DeviceState) => { state = next; for (const listener of listeners) listener(next); };
    /** `unformatted`: the device reports a card with no format, as a new card does. */
    const plug = async (unformatted = false) => {
        publish({ ...idle, status: 'connecting' });
        const info = await rig.client.deviceInfo(), page = await rig.client.listPage({});
        const store = unformatted ? null : { storeId: page.storeId, commitSequence: page.commitSequence };
        publish({ status: 'ready', client: rig.client, store, info, error: null });
        return true;
    };
    const unplug = async () => publish(idle);
    let gate: Promise<void> = Promise.resolve(), open = () => {};
    const list = rig.client.list.bind(rig.client);
    rig.client.list = async (...args) => { await gate; return list(...args); };
    const hold = () => { gate = new Promise(resolve => open = resolve); };
    const release = () => open();
    const offer = (version: string) => { firmwareCheck.release = { version, bytes: 1, sha256: '', url: 'https://updates.test/UPDATE.BIN' }; };
    const watcher: DeviceWatcher = {
        get current() { return state; },
        subscribe(listener) { listeners.add(listener); listener(state); return () => listeners.delete(listener); },
        requestDevice: plug,
        disconnect: unplug,
        close: unplug,
    };
    const session = new WatchedDeviceSession(watcher);
    // `open()` then gives this session, and never a WebUSB one.
    Object.assign(deviceHolder, { session, opening: Promise.resolve(session) });
    Object.assign(window, { deviceTest: { plug, unplug, hold, release, offer } });
}
