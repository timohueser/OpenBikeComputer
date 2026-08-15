/**
 * A {@link DeviceSession} backed by the simulated device — **the dev harness only**.
 *
 * The LM20's USB peripheral does not exist yet (#889), so without this there is no way to click
 * through a map upload, a route drop or a firmware update at all. `loopback.ts` already models the
 * protocol properly (id assignment, dedup, `busy`, the abort handshake, packet-sized bulk reads),
 * so wiring it to a session drives the real UI against a real protocol conversation — the only
 * fiction is the cable.
 *
 * **Why it lives outside `src/`.** C3 drew a hard line: no shipping module may import
 * `lib/usb/loopback`, guarded twice — a source scan in `usb/vectors.test.ts` and a chunk assertion
 * in `usb/bundle.test.ts`. A dev-only dynamic import inside `src/` would satisfy neither, and the
 * chunk guard is right to refuse it: whether such a branch is tree-shaken depends on how the build
 * was invoked (`import.meta.env.DEV` is not `false` when Rollup runs under vitest), so "it gets
 * dropped in production" would be a property nothing in CI actually checks. A separate entry point
 * that no tier's build has as an input is a fact instead of a hope.
 */

import { gpxToObcr } from "../src/lib/convert/bridge";
import { ProtocolClient } from "../src/lib/usb/client";
import { Crc32 } from "../src/lib/usb/crc32";
import { WatchedDeviceSession } from "../src/lib/usb/session.svelte";
import { MockDevice, loopbackLink } from "../src/lib/usb/loopback";
import { encodeRideObject, encodeTripObject, type RideObject, type RidePoint } from "../src/lib/usb/objects";
import type { BytePipe, DeviceLink } from "../src/lib/usb/pipe";
import type { DeviceSession, DeviceState, DeviceWatcher } from "../src/lib/usb/session";

const IDLE: DeviceState = { status: "idle", client: null, identity: null, info: null, error: null };

/**
 * The rate the simulated device moves bytes to and from its card.
 *
 * ~700 KB/s was the retired SPI transport's write ceiling, and it is kept **deliberately
 * pessimistic** rather than re-pinned: the sEMMC pivot (#1158) took the card to 8.2 MB/s raw and the
 * upload pipeline was retuned for it, but nothing end to end has been measured on glass. A harness
 * that promised a number the hardware has not confirmed would be worse than one that is honestly
 * slow. What it has to do is only this: an unthrottled loopback finishes a 100 MB "map" in seconds,
 * which would make every progress bar, rate and remaining-time estimate in the UI untestable.
 *
 * **Both directions**, and for the same reason. Pacing only what the device *receives* left a ride
 * pull (C5 #904) running at memory speed, so its progress bar and Cancel button existed for about
 * four milliseconds — a surface that could not be looked at, let alone driven.
 */
const CARD_BYTES_PER_SECOND = 700 * 1024;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** The device end of a link, paced to {@link CARD_BYTES_PER_SECOND} in both directions. */
function paced(link: DeviceLink): DeviceLink {
    const bulk = link.bulk;
    /**
     * A leaky bucket: the wall-clock instant the card will have finished everything charged to it
     * so far. Charging forward from `max(budgetUntil, now)` rather than from a transfer's start
     * makes the pacing correct across an idle gap — a start-anchored budget goes stale between
     * transfers and lets the next one run unthrottled until it catches up.
     */
    let budgetUntil = 0;

    function charge(bytes: number): Promise<unknown> | null {
        const now = performance.now();
        const start = Math.max(budgetUntil, now);
        budgetUntil = start + (bytes / CARD_BYTES_PER_SECOND) * 1000;
        // Pace in packet-sized debts but skip sub-timer waits: a 512-byte packet is 0.7 ms of card
        // time, well under a `setTimeout`'s resolution, so it is left to accumulate.
        return start - now > 5 ? sleep(start - now) : null;
    }

    const throttledBulk: BytePipe = {
        transport: bulk.transport,
        get open() {
            return bulk.open;
        },
        async read(signal) {
            const slice = await bulk.read(signal);
            await charge(slice.length);
            return slice;
        },
        async write(bytes, signal) {
            await charge(bytes.length);
            await bulk.write(bytes, signal);
        },
        reset: () => {
            budgetUntil = 0;
            return bulk.reset();
        },
        close: () => bulk.close(),
    };
    return { control: link.control, bulk: throttledBulk, close: () => link.close() };
}

/**
 * Rides on the simulated card, so the export panel (C5 #904) has a catalog to render.
 *
 * A device with nothing on it renders one empty-state line, which is not the screen worth looking
 * at. These three are shaped for the cases the panel has to get right rather than for plausibility:
 * an 11-hour ride with sensors — long enough on the wire that the progress bar, the rate and the
 * Cancel button are real rather than a flash — a short one without, and one recorded before any
 * peer set the clock, which the device reports as `start_time = 0` and the panel must not render
 * as 1970.
 */
function seedRides(device: MockDevice): void {
    const rides: Array<{ id: number; ride: RideObject }> = [
        { id: 3, ride: syntheticRide("Schauinsland & back", 1_783_598_400, 40_000, true) },
        { id: 5, ride: syntheticRide("Kaiserstuhl loop", 1_783_339_200, 1_100, false) },
        { id: 6, ride: syntheticRide("Shakedown", 0, 320, false) },
    ];
    for (const { id, ride } of rides) {
        const bytes = encodeRideObject(ride);
        device.seedRide(
            {
                objectId: id,
                byteLen: bytes.length,
                startTime: ride.startTime,
                distanceM: ride.distanceM,
                movingTimeS: ride.movingTimeS,
                avgSpeedCms: ride.avgSpeedCms,
                climbM: ride.climbM,
                name: ride.name,
            },
            bytes,
        );
    }
}

/**
 * A route with named, categorized waypoints on the simulated card, so the chart-room preview
 * (waypoint card, map diamonds, profile ticks) can be exercised end to end.
 *
 * The OBCR is **real**: generated at connect time by the same wasm bridge a dropped GPX goes
 * through, from the deterministic inline GPX below — not hand-forged bytes, so the preview's
 * read-back (`routeTrack` + `routeWaypoints`) decodes exactly what the converter stores, waypoint
 * placement and all. The wasm module is local to the bundle, so this stays offline-safe; it is
 * the same prerequisite the harness's route-drop flow already has. The catalog metrics are read
 * out of the OBCR's own header (spec §1) rather than typed in twice.
 */
async function seedWaypointRoute(device: MockDevice): Promise<void> {
    try {
        await seedGpxRoute(device, 1, "Kaiserstuhl loop", kaiserstuhlGpx());
    } catch (cause) {
        // A missing wasm artifact breaks route drops too; keep the rest of the harness usable.
        console.warn("dev-harness: could not seed the waypoint route (is the wasm bridge built?)", cause);
    }
}

/** Convert one inline GPX through the real wasm bridge and put it on the card under `id`,
 *  catalog metrics read out of the OBCR's own header (spec §1). Returns the header's totals. */
async function seedGpxRoute(
    device: MockDevice,
    id: number,
    name: string,
    gpx: string,
): Promise<{ distanceM: number; ascentM: number }> {
    const bytes = await gpxToObcr(new TextEncoder().encode(gpx), name);
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const distanceM = view.getUint32(36, true);
    const ascentM = view.getUint32(40, true);
    device.seedRoute(
        {
            objectId: id,
            byteLen: bytes.length,
            distanceM,
            ascentM,
            pointCount: view.getUint32(32, true),
            waypointCount: view.getUint16(116, true),
            name,
            crc32: Crc32.of(bytes),
            expiresAt: 0,
            retention: 0,
        },
        bytes,
    );
    return { distanceM, ascentM };
}

/**
 * A three-stage tour on the simulated card, so the trip band and its combined preview — the
 * multi-color map, the concatenated elevation profile with its stage seams, the merged waypoint
 * card — can be exercised end to end without dropping three GPX files first.
 *
 * The stages are contiguous (each starts where the last ended), every stage has a real elevation
 * shape, and two of the three carry waypoints — so the merged card shows cumulative distances
 * across a stage that contributes none. The trip object and its catalog entry are seeded the way
 * the device would serve them: totals summed over the resolvable stages.
 */
async function seedTour(device: MockDevice): Promise<void> {
    try {
        const stages: Array<{ id: number; spec: TourLeg }> = [
            { id: 2, spec: TOUR_LEGS[0] },
            { id: 3, spec: TOUR_LEGS[1] },
            { id: 4, spec: TOUR_LEGS[2] },
        ];
        let distanceM = 0;
        let ascentM = 0;
        for (const { id, spec } of stages) {
            const totals = await seedGpxRoute(device, id, spec.name, legGpx(spec));
            distanceM += totals.distanceM;
            ascentM += totals.ascentM;
        }
        const name = "Black Forest traverse";
        const bytes = encodeTripObject({ name, stages: stages.map((s) => s.id) });
        device.seedTrip(
            {
                objectId: 1,
                byteLen: bytes.length,
                totalDistanceM: distanceM,
                totalAscentM: ascentM,
                stageCount: stages.length,
                name,
                crc32: Crc32.of(bytes),
            },
            bytes,
        );
    } catch (cause) {
        console.warn("dev-harness: could not seed the tour (is the wasm bridge built?)", cause);
    }
}

/** One leg of the seeded tour: endpoints, an elevation shape over the leg, waypoints at
 *  fractions of it. */
interface TourLeg {
    name: string;
    from: [number, number];
    to: [number, number];
    /** Metres at `t` ∈ [0, 1] along the leg. */
    ele: (t: number) => number;
    wpts: Array<{ t: number; name: string; sym: string }>;
}

const TOUR_LEGS: TourLeg[] = [
    {
        name: "Freiburg → Belchen",
        from: [47.995, 7.85],
        to: [47.822, 7.836],
        // Valley start, one foothill, then the long climb to the Belchen shoulder.
        ele: (t) => 280 + 180 * Math.max(0, Math.sin(t * Math.PI * 2)) * (1 - t) + 1050 * t * t,
        wpts: [
            { t: 0.1, name: "Bäckerei Krachenfels", sym: "Bakery" },
            { t: 0.62, name: "Brunnen Münstertal", sym: "Drinking Water" },
        ],
    },
    {
        name: "Belchen → Feldberg",
        from: [47.822, 7.836],
        to: [47.874, 8.004],
        // Ridge riding: high the whole way, two saddles between three crests.
        ele: (t) => 1150 + 240 * Math.sin(t * Math.PI * 3 + 0.3) * Math.sin(t * Math.PI),
        wpts: [{ t: 0.5, name: "Aussicht Wiedener Eck", sym: "Viewpoint" }],
    },
    {
        name: "Feldberg → Titisee",
        from: [47.874, 8.004],
        to: [47.903, 8.152],
        // The long descent to the lake, one counter-climb midway. No waypoints on purpose.
        ele: (t) => 1380 - 620 * t + 120 * Math.max(0, Math.sin(t * Math.PI * 2 + 0.5)) * (1 - t),
        wpts: [],
    },
];

/** Render one leg as GPX: 60 segments of a gently winding line between the endpoints, waypoints
 *  snapped onto track points so their pinned `distAlongM` is exact. */
function legGpx(leg: TourLeg): string {
    const n = 60;
    const coord = (t: number): [number, number] => [
        leg.from[0] + (leg.to[0] - leg.from[0]) * t + 0.006 * Math.sin(t * Math.PI * 3),
        leg.from[1] + (leg.to[1] - leg.from[1]) * t + 0.008 * Math.sin(t * Math.PI * 2 + 1),
    ];
    const points: string[] = [];
    for (let i = 0; i <= n; i++) {
        const t = i / n;
        const [lat, lon] = coord(t);
        points.push(
            `<trkpt lat="${lat.toFixed(6)}" lon="${lon.toFixed(6)}"><ele>${leg.ele(t).toFixed(1)}</ele></trkpt>`,
        );
    }
    const waypoints = leg.wpts
        .map((w) => {
            const [lat, lon] = coord(Math.round(w.t * n) / n);
            return `<wpt lat="${lat.toFixed(6)}" lon="${lon.toFixed(6)}"><name>${w.name}</name><sym>${w.sym}</sym></wpt>`;
        })
        .join("\n  ");
    return `<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="obc-dev-harness">
  ${waypoints}
  <trk><name>${leg.name}</name><trkseg>${points.join("")}</trkseg></trk>
</gpx>
`;
}

/**
 * A ~15 km loop around the Kaiserstuhl with a real elevation shape (so the profile zoom has
 * something to zoom into) and four `<wpt>`s spanning the symbol table: resupply, water, a
 * deliberately-unmapped Viewpoint (generic), and a campsite.
 */
function kaiserstuhlGpx(): string {
    const points: string[] = [];
    const n = 72;
    for (let i = 0; i <= n; i++) {
        const a = (i / n) * 2 * Math.PI;
        const lat = 48.09 + 0.024 * Math.sin(a);
        const lon = 7.66 + 0.036 * Math.cos(a);
        const ele = 210 + 190 * Math.max(0, Math.sin(a * 1.5 + 0.4)) + 30 * Math.sin(a * 4);
        points.push(`<trkpt lat="${lat.toFixed(6)}" lon="${lon.toFixed(6)}"><ele>${ele.toFixed(1)}</ele></trkpt>`);
    }
    const waypoints = [
        { lat: 48.0902, lon: 7.6962, name: "Bäckerei Lieb", sym: "Bakery" },
        { lat: 48.1128, lon: 7.6631, name: "Wasserstelle Vogtsburg", sym: "Drinking Water" },
        { lat: 48.0932, lon: 7.6238, name: "Aussicht Totenkopf", sym: "Viewpoint" },
        { lat: 48.0669, lon: 7.6489, name: "Zeltplatz option", sym: "Campground" },
    ]
        .map((w) => `<wpt lat="${w.lat}" lon="${w.lon}"><name>${w.name}</name><sym>${w.sym}</sym></wpt>`)
        .join("\n  ");
    return `<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="obc-dev-harness">
  ${waypoints}
  <trk><name>Kaiserstuhl loop</name><trkseg>${points.join("")}</trkseg></trk>
</gpx>
`;
}

/** One second-per-point ride climbing gently out of Freiburg. */
function syntheticRide(name: string, startTime: number, points: number, sensors: boolean): RideObject {
    const list: RidePoint[] = [];
    for (let i = 0; i < points; i++) {
        list.push({
            tOffsetS: i,
            lat1e7: 479_950_000 + i * 120,
            lon1e7: 78_420_000 + i * 90,
            eleM: 280 + Math.round(600 * Math.sin((i / points) * Math.PI)),
            hrBpm: sensors ? 128 + (i % 22) : null,
            cadenceRpm: sensors ? 74 + (i % 11) : null,
            powerW: sensors ? 180 + (i % 60) : null,
        });
    }
    return {
        version: 2,
        name,
        startTime,
        distanceM: points * 7,
        movingTimeS: points,
        avgSpeedCms: 700,
        climbM: 600,
        avgHr: sensors ? 139 : null,
        maxHr: sensors ? 171 : null,
        avgCadence: sensors ? 79 : null,
        avgPower: sensors ? 209 : null,
        maxPower: sensors ? 410 : null,
        points: list,
    };
}

/** A watcher whose "device" is an in-memory one. The same three methods the WebUSB watcher has. */
class LoopbackWatcher implements DeviceWatcher {
    private state: DeviceState = IDLE;
    private readonly listeners = new Set<(state: DeviceState) => void>();
    private open: { device: MockDevice; close: () => Promise<void> } | null = null;

    get current(): DeviceState {
        return this.state;
    }

    subscribe(listener: (state: DeviceState) => void): () => void {
        this.listeners.add(listener);
        listener(this.state);
        return () => this.listeners.delete(listener);
    }

    /** Stands in for the browser's chooser, so the connect button is exercised exactly as it will
     *  be against hardware — including the rule that it only runs from a real click. */
    async requestDevice(): Promise<boolean> {
        if (this.open) return true;
        this.publish({ ...IDLE, status: "connecting" });
        const link = loopbackLink();
        const device = new MockDevice(paced(link.device));
        seedRides(device);
        await seedWaypointRoute(device);
        await seedTour(device);
        void device.run();
        const client = new ProtocolClient(link.host);
        this.open = {
            device,
            close: async () => {
                device.stop();
                await client.close();
                await link.device.close();
            },
        };
        const identity = await client.identity();
        const info = await client.deviceInfo();
        this.publish({ status: "ready", client, identity, info, error: null });
        return true;
    }

    async disconnect(): Promise<void> {
        const open = this.open;
        this.open = null;
        await open?.close();
        this.publish(IDLE);
    }

    async close(): Promise<void> {
        await this.disconnect();
        this.listeners.clear();
    }

    private publish(state: DeviceState): void {
        this.state = state;
        for (const listener of this.listeners) listener(state);
    }
}

/** Open a session over the simulated device. Nothing is connected until `requestDevice()`. */
export async function openSimulatedSession(): Promise<DeviceSession> {
    return new WatchedDeviceSession(new LoopbackWatcher(), "loopback");
}
