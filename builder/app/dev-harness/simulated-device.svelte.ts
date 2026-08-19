/**
 * A {@link DeviceSession} backed by the simulated device — **the dev harness only**.
 *
 * The LM20's USB peripheral does not exist yet (#889), so without this there is no way to click
 * through a map upload, a route drop or a firmware update at all. `loopback.ts` already models
 * protocol v4 properly (id allocation, compare-and-swap on revisions, `busy`, the cancel handshake,
 * paged `LIST`s and packet-sized record reads), so wiring it to a session drives the real UI
 * against a real protocol conversation — the only fiction is the cable.
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
import { FlatStoreClient } from "../src/lib/usb/client";
import { WatchedDeviceSession } from "../src/lib/usb/session.svelte";
import { MockDevice, loopbackLink } from "../src/lib/usb/loopback";
import { encodeRideObject, encodeTripObject, type RideObject, type RidePoint } from "../src/lib/usb/objects";
import type { BytePipe, DeviceLink } from "../src/lib/usb/pipe";
import { EntryFlags, ObjectKind } from "../src/lib/usb/protocol";
import type { DeviceSession, DeviceState, DeviceWatcher } from "../src/lib/usb/session";

const IDLE: DeviceState = { status: "idle", client: null, store: null, info: null, error: null };

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

/**
 * The device end of a link, paced to {@link CARD_BYTES_PER_SECOND} in both directions.
 *
 * Only the **stream** channel is paced (§5.2's second endpoint pair): it is where payload bytes
 * move, and it is the one whose speed any progress bar is a picture of. The control channel carries
 * §3's request and response frames — a hundred bytes each, at most one in flight — so throttling it
 * would only add latency to a `LIST` without making any surface more testable.
 */
function paced(link: DeviceLink): DeviceLink {
    const stream = link.stream;
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

    const throttledStream: BytePipe = {
        transport: stream.transport,
        get open() {
            return stream.open;
        },
        async read(signal) {
            const slice = await stream.read(signal);
            await charge(slice.length);
            return slice;
        },
        async write(bytes, signal) {
            await charge(bytes.length);
            await stream.write(bytes, signal);
        },
        reset: () => {
            budgetUntil = 0;
            return stream.reset();
        },
        close: () => stream.close(),
    };
    return { control: link.control, stream: throttledStream, close: () => link.close() };
}

/**
 * The ids the seeds take.
 *
 * An `ObjectId` is **store-global** (`FLAT_Store_Format.md` §3): one allocation cursor for every
 * kind, never reused. So the routes, the trip and the rides here share one numbering — under the v1
 * wire each type had its own id space and a route 3 could sit beside a ride 3, which is exactly the
 * assumption a harness would otherwise carry forward into a screen nobody could reproduce on glass.
 */
const ROUTE_IDS = { kaiserstuhl: 1n, leg1: 2n, leg2: 3n, leg3: 4n } as const;
const TRIP_ID = 5n;
const RIDE_IDS = { long: 6n, short: 7n, noClock: 8n, recording: 9n } as const;

/**
 * Rides on the simulated card, so the ride surfaces have a catalog to render.
 *
 * A device with nothing on it renders one empty-state line, which is not the screen worth looking
 * at. These are shaped for the cases the surfaces have to get right rather than for plausibility:
 * an 11-hour ride with sensors — long enough on the wire that the progress bar, the rate and the
 * Cancel button are real rather than a flash — a short one without, one recorded before any peer
 * set the clock, and one the device is **still recording**.
 *
 * The last is a metadata-only row on purpose. §3.5 refuses a `GET` of an entry carrying `RECORDING`
 * — its payload length and CRC are zero until the commit that ends the ride — so seeding it with no
 * bytes is what the device really holds, and it is what makes the "listed, not offered" path
 * something a developer can look at rather than reason about.
 */
function seedRides(device: MockDevice): void {
    const rides: Array<{ id: bigint; ride: RideObject }> = [
        { id: RIDE_IDS.long, ride: syntheticRide("Schauinsland & back", 1_783_598_400, 40_000, true) },
        { id: RIDE_IDS.short, ride: syntheticRide("Kaiserstuhl loop", 1_783_339_200, 1_100, false) },
        { id: RIDE_IDS.noClock, ride: syntheticRide("Shakedown", 0, 320, false) },
    ];
    for (const { id, ride } of rides) {
        device.seed({
            objectId: id,
            kind: ObjectKind.Ride,
            displayName: ride.name,
            bytes: encodeRideObject(ride),
        });
    }
    device.seed({
        objectId: RIDE_IDS.recording,
        kind: ObjectKind.Ride,
        displayName: "Today",
        flags: EntryFlags.Recording,
    });
}

/**
 * A route with named, categorized waypoints on the simulated card, so the chart-room preview
 * (waypoint card, map diamonds, profile ticks) can be exercised end to end.
 *
 * The OBCR is **real**: generated at connect time by the same wasm bridge a dropped GPX goes
 * through, from the deterministic inline GPX below — not hand-forged bytes, so the preview's
 * read-back (`routeTrack` + `routeWaypoints`) decodes exactly what the converter stores, waypoint
 * placement and all. The wasm module is local to the bundle, so this stays offline-safe; it is
 * the same prerequisite the harness's route-drop flow already has.
 */
async function seedWaypointRoute(device: MockDevice): Promise<void> {
    try {
        await seedGpxRoute(device, ROUTE_IDS.kaiserstuhl, "Kaiserstuhl loop", kaiserstuhlGpx());
    } catch (cause) {
        // A missing wasm artifact breaks route drops too; keep the rest of the harness usable.
        console.warn("dev-harness: could not seed the waypoint route (is the wasm bridge built?)", cause);
    }
}

/**
 * Convert one inline GPX through the real wasm bridge and put it on the card under `id`.
 *
 * The catalog entry is the whole of what a `LIST` carries — id, revision, payload length, payload
 * CRC, kind, flags and a display name (§3.3) — and `MockDevice.seed` derives the length and the CRC
 * from the bytes, so there is nothing else to state. A route's distance, ascent and point count are
 * *payload* facts, in the OBCR header, and no seed here has to repeat them.
 */
async function seedGpxRoute(device: MockDevice, id: bigint, name: string, gpx: string): Promise<void> {
    const bytes = await gpxToObcr(new TextEncoder().encode(gpx), name);
    device.seed({ objectId: id, kind: ObjectKind.Route, displayName: name, bytes });
}

/**
 * A three-stage tour on the simulated card, so the trip band and its combined preview — the
 * multi-color map, the concatenated elevation profile with its stage seams, the merged waypoint
 * card — can be exercised end to end without dropping three GPX files first.
 *
 * The stages are contiguous (each starts where the last ended), every stage has a real elevation
 * shape, and two of the three carry waypoints — so the merged card shows cumulative distances
 * across a stage that contributes none.
 *
 * The trip object names its stages in 16 bits (`objects.ts`), which is why the stage ids are
 * narrowed to `Number` here: the ids are the store's `u64`s and this format is the one place they
 * do not fit.
 */
async function seedTour(device: MockDevice): Promise<void> {
    try {
        const stages: Array<{ id: bigint; spec: TourLeg }> = [
            { id: ROUTE_IDS.leg1, spec: TOUR_LEGS[0] },
            { id: ROUTE_IDS.leg2, spec: TOUR_LEGS[1] },
            { id: ROUTE_IDS.leg3, spec: TOUR_LEGS[2] },
        ];
        for (const { id, spec } of stages) {
            await seedGpxRoute(device, id, spec.name, legGpx(spec));
        }
        const name = "Black Forest traverse";
        device.seed({
            objectId: TRIP_ID,
            kind: ObjectKind.Trip,
            displayName: name,
            bytes: encodeTripObject({ name, stages: stages.map((s) => Number(s.id)) }),
        });
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
        const client = new FlatStoreClient(link.host);
        this.open = {
            device,
            close: async () => {
                device.stop();
                await client.close();
                await link.device.close();
            },
        };
        // The same two reads `WebUsbWatcher.connect` makes, in the same order: §5.2.1's strings over
        // EP0, then the `LIST` page whose prefix carries the store's identity (§3.3).
        const info = await client.deviceInfo();
        const page = await client.listPage({});
        this.publish({
            status: "ready",
            client,
            store: { storeId: page.storeId, commitSequence: page.commitSequence },
            info,
            error: null,
        });
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
