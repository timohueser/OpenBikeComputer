/**
 * A device on a pretend cable: `planner.html?fake-device`, on the dev server and in the build of the browser tests. The wasm
 * test device of the sync tests holds a map, a route and two rides of today. The chooser of the Connect button plugs it in.
 * `window.deviceTest`: `plug(unformatted)` and `unplug()` move the cable, `hold()` stops each sync at its start and `holdPut()` each put before its first byte, until `release()`; `map()` gives the map on the card; and
 * `offer(version)` publishes a pretend software version. The card is the same at each page load. `plug(true)` puts a second
 * device with the same serial on the cable, whose card has no format. A format restarts the device, as the real one does.
 */
import { initCore } from '../../src/lib/core/bridge';
import { deviceHolder } from '../../src/lib/device/session.svelte';
import { firmwareCheck } from '../../src/lib/firmware/check.svelte';
import { flatDevice, initFlatDevice } from '../../src/lib/usb/flat-device';
import { encodeRideObject, type RideObject } from '../../src/lib/usb/objects';
import { ObjectKind } from '../../src/lib/usb/protocol';
import { bytesSource } from '../../src/lib/usb/source';
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

const STORE = '0f5e9a7c2b3d4e1f8a6b5c4d3e2f1a0b';

export async function fakeDevice(): Promise<void> {
    await Promise.all([initFlatDevice(), initCore()]);
    const deviceInfo = { firmwareRevision: '0.9.2', hardwareRevision: 'obc-lm20-r1', serialNumber: '9F3A00C1E2B44F2A' };
    const rig = flatDevice({ storeId: STORE, deviceInfo }), blank = flatDevice({ formatted: false, deviceInfo });
    const now = Math.floor(Date.now() / 1000);
    rig.device.seed({ kind: ObjectKind.MapShard, displayName: 'Baden-Württemberg', bytes: new Uint8Array(40_000) });
    const route = rig.device.seed({ kind: ObjectKind.Route, displayName: 'Lake loop', bytes: new Uint8Array(1_200) });
    rig.device.seed({ kind: ObjectKind.Ride, displayName: 'ride', bytes: encodeRideObject(ride(now - 7200, 8_000_000)) });
    rig.device.seed({ kind: ObjectKind.Ride, displayName: 'ride', bytes: encodeRideObject(ride(now - 3600, 8_050_000)) });

    const idle: DeviceState = { status: 'idle', client: null, store: null, info: null, error: null };
    let state = idle;
    const listeners = new Set<(state: DeviceState) => void>();
    const publish = (next: DeviceState) => { state = next; for (const listener of listeners) listener(next); };
    /** `unformatted`: the device with the new card. A card with no format has no list, so the session has no store. */
    const plug = async (unformatted = false) => {
        const { client } = unformatted ? blank : rig;
        publish({ ...idle, status: 'connecting' });
        const info = await client.deviceInfo(), page = await client.listPage({}).catch(() => null);
        publish({ status: 'ready', client, store: page && { storeId: page.storeId, commitSequence: page.commitSequence }, info, error: null });
        return true;
    };
    const unplug = async () => publish(idle);
    for (const [{ client }, unformatted] of [[rig, false], [blank, true]] as const) {
        const format = client.format.bind(client);
        client.format = async (...args) => {
            const answer = await format(...args);
            setTimeout(() => { publish(idle); setTimeout(() => void plug(unformatted), 600); }, 300);
            return answer;
        };
    }
    let gate: Promise<void> = Promise.resolve(), open = () => {};
    const list = rig.client.list.bind(rig.client);
    rig.client.list = async (...args) => { await gate; return list(...args); };
    const hold = () => { gate = new Promise(resolve => open = resolve); };
    // A held put has started on the wire: the device has its header and waits for the first byte.
    let putGate: Promise<void> = Promise.resolve(), openPut = () => {};
    const put = rig.client.put.bind(rig.client);
    rig.client.put = (target, source, options) => {
        const src = source instanceof Uint8Array ? bytesSource(source) : source, wait = putGate;
        return put(target, { ...src, async *chunks(size) { await wait; yield* src.chunks(size); } }, options);
    };
    const holdPut = () => { putGate = new Promise(resolve => openPut = resolve); };
    const release = () => { open(); openPut(); };
    /** The payload of the map that the device selects, as numbers. */
    const map = () => {
        const shards = rig.device.entries.filter(entry => entry.kind === ObjectKind.MapShard);
        const selected = shards.reduce((best, entry) => entry.objectId < best.objectId ? entry : best);
        return Array.from(rig.device.payloadOf(selected.objectId) ?? []);
    };
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
    // `route`: a `sent` entry for the route on the card. Its key is not the `sentKey` of any route.
    Object.assign(window, { deviceTest: { plug, unplug, hold, holdPut, release, map, offer, route: { key: 'old', card: STORE, days: [String(route.objectId)], trip: null } } });
}
